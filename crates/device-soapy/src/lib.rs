use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_device::{
    DeviceDriver, DeviceError, DuplexState, RxSink, Sample, SdrDevice, TxStream, Worker, lock,
};
use sdrmm_wire::{
    Capabilities, ChannelCapabilities, DeviceInfo, DeviceSettings, Direction as WireDirection,
    DirectionalCapabilities, GainStage, GainValue, StreamSettings,
};
use soapysdr::{Direction, ErrorCode};

mod caps;
mod probe;
mod runtime;
mod watchdog;

pub use probe::enable_isolated_probes;
pub use runtime::{RuntimeInfo, configure_bundled_runtime, runtime_info};
use watchdog::{Watch, Watchdog};

const DRIVER_ID: &str = "soapy";
const READ_TIMEOUT_US: i64 = 100_000;
const MIN_BLOCK: usize = 8192;
const OVERFLOW_LOG_EVERY: u64 = 1000;
const GAIN_MODE_SETTING: &str = "gain_mode";

static ENUMERATE_LOCK: Mutex<()> = Mutex::new(());

fn enumerate_serialized(filter: &str) -> Result<Vec<soapysdr::Args>, soapysdr::Error> {
    let _guard = ENUMERATE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    soapysdr::enumerate(filter)
}

const BUSY_HINTS: [&str; 6] = [
    "busy",
    "in use",
    "claim",
    "access denied",
    "libusb_error_access",
    "unable to open",
];

fn reads_as_busy(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    BUSY_HINTS.iter().any(|hint| message.contains(hint))
}

fn map_err(error: soapysdr::Error) -> DeviceError {
    let message = error.to_string();
    match error.code {
        ErrorCode::NotSupported => DeviceError::Unsupported(message),
        _ if reads_as_busy(&message) => DeviceError::InUse(message),
        _ => DeviceError::Io(message),
    }
}

fn args_key(args: &soapysdr::Args) -> String {
    args.get("serial").map_or_else(
        || args.to_string(),
        |serial| match args.get("mode") {
            Some(mode) => format!("{serial}@{mode}"),
            None => serial.to_string(),
        },
    )
}

fn device_info(args: &soapysdr::Args) -> DeviceInfo {
    let serial = args.get("serial").map(str::to_string);
    let label = args.get("label").map_or_else(
        || {
            let driver = args.get("driver").unwrap_or(DRIVER_ID);
            serial
                .as_ref()
                .map_or_else(|| driver.to_string(), |serial| format!("{driver} {serial}"))
        },
        str::to_string,
    );
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: args_key(args),
        label,
        serial,
        profile: None,
    }
}

#[derive(Clone, Debug)]
struct ProbeIdentity {
    filter: String,
    key: String,
}

impl ProbeIdentity {
    fn from_args(args: &soapysdr::Args) -> Self {
        let filter = match (args.get("driver"), args.get("serial"), args.get("mode")) {
            (Some(driver), Some(serial), Some(mode)) => {
                format!("driver={driver},serial={serial},mode={mode}")
            }
            (Some(driver), Some(serial), None) => format!("driver={driver},serial={serial}"),
            (Some(driver), None, _) => format!("driver={driver}"),
            _ => args.to_string(),
        };
        Self {
            filter,
            key: args_key(args),
        }
    }

    fn is_present(&self) -> Result<bool, DeviceError> {
        Ok(probe::devices(&self.filter, probe::Scope::Deep)?
            .iter()
            .any(|found| found.info.key == self.key))
    }
}

#[derive(Default)]
pub struct SoapyDriver {
    excluded: Vec<String>,
}

impl SoapyDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hides the named SoapySDR drivers, for hardware this build speaks to natively. Excluding a
    /// driver here rather than deduplicating afterwards is what keeps a dongle off the list
    /// exactly once: the factory serial most RTL-SDRs ship with is not unique, so the registry's
    /// serial merge cannot tell two of them apart.
    #[must_use]
    pub fn excluding<S: Into<String>>(drivers: impl IntoIterator<Item = S>) -> Self {
        Self {
            excluded: drivers.into_iter().map(Into::into).collect(),
        }
    }

    fn hides(&self, found: &probe::Found) -> bool {
        self.excluded.iter().any(|driver| found.is_driver(driver))
    }

    fn visible(&self, scope: probe::Scope) -> Result<Vec<probe::Found>, DeviceError> {
        Ok(probe::devices("", scope)?
            .into_iter()
            .filter(|found| !self.hides(found))
            .collect())
    }

    fn listed(&self, scope: probe::Scope) -> Vec<DeviceInfo> {
        match self.visible(scope) {
            Ok(found) => found.into_iter().map(|found| found.info).collect(),
            Err(error) => {
                tracing::warn!("soapy enumerate failed: {error}");
                Vec::new()
            }
        }
    }
}

impl DeviceDriver for SoapyDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        self.listed(probe::Scope::Fast)
    }

    fn probe_deep(&self) -> Vec<DeviceInfo> {
        self.listed(probe::Scope::Deep)
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let found = self
            .visible(probe::Scope::Deep)?
            .into_iter()
            .find(|found| found.info.key == info.key)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
        let args = soapysdr::Args::from(found.args.as_str());
        let identity = ProbeIdentity::from_args(&args);
        let device = soapysdr::Device::new(args).map_err(map_err)?;
        Ok(Box::new(SoapyDevice::from_device(device, identity)?))
    }
}

fn args_map(args: &soapysdr::Args) -> BTreeMap<String, String> {
    args.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn optional<T: Default>(label: &str, result: Result<T, soapysdr::Error>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(capability = label, "soapy capability unavailable: {error}");
            T::default()
        }
    }
}

fn optional_value<T>(label: &str, result: Result<T, soapysdr::Error>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(capability = label, "soapy capability unavailable: {error}");
            None
        }
    }
}

fn query_channel(
    device: &soapysdr::Device,
    direction: Direction,
    channel: usize,
) -> Result<ChannelCapabilities, DeviceError> {
    let rate_ranges = device
        .get_sample_rate_range(direction, channel)
        .map_err(map_err)?;
    let (sample_rates, sample_rate_ranges) = caps::rate_capabilities(&rate_ranges);
    let mut gains = Vec::new();
    for name in device.list_gains(direction, channel).map_err(map_err)? {
        let range = device
            .gain_element_range(direction, channel, name.as_str())
            .map_err(map_err)?;
        gains.push(GainStage {
            name,
            range: caps::ranges(&[range])[0],
            values: Vec::new(),
        });
    }
    let frequency_components = optional(
        "frequency components",
        device.list_frequencies(direction, channel),
    );
    let info = args_map(&device.channel_info(direction, channel).map_err(map_err)?);
    let formats = optional("stream formats", device.stream_formats(direction, channel));
    let native = optional_value(
        "native stream format",
        device.native_stream_format(direction, channel),
    );
    Ok(ChannelCapabilities {
        channel: u32::try_from(channel).map_err(|_| {
            DeviceError::Unsupported(format!("Soapy channel index {channel} exceeds u32"))
        })?,
        freq_ranges: caps::ranges(
            &device
                .frequency_range(direction, channel)
                .map_err(map_err)?,
        ),
        sample_rates,
        sample_rate_ranges,
        bandwidth_ranges: caps::ranges(
            &device
                .bandwidth_range(direction, channel)
                .map_err(map_err)?,
        ),
        gains,
        antennas: device.antennas(direction, channel).map_err(map_err)?,
        gain_mode: optional("gain mode", device.has_gain_mode(direction, channel)),
        dc_offset_mode: optional(
            "dc offset mode",
            device.has_dc_offset_mode(direction, channel),
        ),
        iq_balance: optional("IQ balance", device.has_iq_balance(direction, channel)),
        full_duplex: optional("full duplex", device.full_duplex(direction, channel)),
        stream_formats: formats.iter().map(ToString::to_string).collect(),
        native_stream_format: native.map(|(format, _)| format.to_string()),
        stream_args: caps::argument_infos(&optional(
            "stream arguments",
            device.stream_args_info(direction, channel),
        )),
        frequency_args: caps::argument_infos(&optional(
            "frequency arguments",
            device.frequency_args_info(direction, channel),
        )),
        frequency_components,
        settings: caps::argument_infos(&optional(
            "channel settings",
            device.channel_setting_info(direction, channel),
        )),
        info,
    })
}

fn query_direction(
    device: &soapysdr::Device,
    direction: Direction,
) -> Result<Vec<ChannelCapabilities>, DeviceError> {
    let count = device.num_channels(direction).map_err(map_err)?;
    (0..count)
        .map(|channel| query_channel(device, direction, channel))
        .collect()
}

fn query_capabilities(device: &soapysdr::Device) -> Result<Capabilities, DeviceError> {
    let rx = query_direction(device, Direction::Rx)?;
    let tx = query_direction(device, Direction::Tx)?;
    if rx.is_empty() && tx.is_empty() {
        return Err(DeviceError::Unsupported(
            "Soapy device reports no RX or TX channels".to_string(),
        ));
    }
    let hardware_time = optional("hardware time", device.has_hardware_time(None));
    let directional = DirectionalCapabilities {
        rx,
        tx,
        device_settings: caps::argument_infos(&device.setting_info().map_err(map_err)?),
        clock_sources: optional("clock sources", device.list_clock_sources()),
        time_sources: optional("time sources", device.list_time_sources()),
        clock_source: device
            .get_clock_source()
            .ok()
            .filter(|value| !value.is_empty()),
        time_source: device
            .get_time_source()
            .ok()
            .filter(|value| !value.is_empty()),
        hardware_time,
        hardware_time_ns: hardware_time
            .then(|| device.get_hardware_time(None).ok())
            .flatten(),
        master_clock_rate: device.get_master_clock_rate().ok(),
        hardware_info: args_map(&device.hardware_info().map_err(map_err)?),
    };
    let mut capabilities = caps::capabilities(directional);
    let gain_mode = capabilities
        .directional
        .as_ref()
        .and_then(|directional| directional.rx.first())
        .is_some_and(|channel| channel.gain_mode);
    if gain_mode
        && !capabilities
            .extra
            .iter()
            .any(|setting| setting.name() == GAIN_MODE_SETTING)
    {
        capabilities.extra.push(sdrmm_wire::ExtraSetting::Bool {
            name: GAIN_MODE_SETTING.to_string(),
            default: device.gain_mode(Direction::Rx, 0).unwrap_or(false),
        });
    }
    Ok(capabilities)
}

fn read_channel_settings(
    device: &soapysdr::Device,
    channel: &ChannelCapabilities,
) -> DeviceSettings {
    let index = channel.channel as usize;
    let mut settings = DeviceSettings {
        center_hz: device.frequency(Direction::Rx, index).ok(),
        sample_rate: device.sample_rate(Direction::Rx, index).ok(),
        antenna: device.antenna(Direction::Rx, index).ok(),
        bandwidth: device
            .bandwidth(Direction::Rx, index)
            .ok()
            .filter(|bandwidth| *bandwidth > 0.0),
        ..DeviceSettings::default()
    };
    for stage in &channel.gains {
        if let Ok(value_db) = device.gain_element(Direction::Rx, index, stage.name.as_str()) {
            settings.gains.push(GainValue {
                stage: stage.name.clone(),
                value_db,
            });
        }
    }
    settings
}

fn read_settings(device: &soapysdr::Device, capabilities: &Capabilities) -> DeviceSettings {
    let Some(directional) = &capabilities.directional else {
        return DeviceSettings::default();
    };
    let Some(primary) = directional.rx.first() else {
        return DeviceSettings::default();
    };
    let mut settings = read_channel_settings(device, primary);
    for channel in directional.rx.iter().skip(1) {
        let channel_settings = read_channel_settings(device, channel);
        settings.streams.push(StreamSettings {
            stream: channel.channel,
            center_hz: channel_settings.center_hz,
            gains: channel_settings.gains,
            antenna: channel_settings.antenna,
        });
    }
    for extra in &capabilities.extra {
        let name = extra.name();
        let read = if name == GAIN_MODE_SETTING {
            device
                .gain_mode(Direction::Rx, 0)
                .map(|value| value.to_string())
        } else {
            device.read_setting(name)
        };
        if let Ok(value) = read {
            let value = match extra {
                sdrmm_wire::ExtraSetting::Bool { .. } => serde_json::Value::Bool(matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "1"
                )),
                sdrmm_wire::ExtraSetting::Range { .. } => value.parse::<f64>().map_or_else(
                    |_| serde_json::Value::String(value),
                    serde_json::Value::from,
                ),
                _ => serde_json::Value::String(value),
            };
            settings.extra.push(sdrmm_wire::ExtraValue {
                name: name.to_string(),
                value,
            });
        }
    }
    settings
}

const RATE_COERCION_TOLERANCE: f64 = 1e-6;

fn rate_was_coerced(requested: f64, actual: f64) -> bool {
    requested.is_finite() && actual.is_finite() && requested != 0.0 && {
        ((actual - requested) / requested).abs() > RATE_COERCION_TOLERANCE
    }
}

fn warn_coerced_rate(requested: Option<f64>, actual: Option<f64>) {
    let (Some(requested), Some(actual)) = (requested, actual) else {
        return;
    };
    if rate_was_coerced(requested, actual) {
        tracing::warn!(
            requested_hz = requested,
            actual_hz = actual,
            "device would not take the requested sample rate; \
             the rate it reports is the one everything downstream is built on"
        );
    }
}

pub struct SoapyDevice {
    device: soapysdr::Device,
    capabilities: Capabilities,
    settings: DeviceSettings,
    identity: ProbeIdentity,
    worker: Worker,
    duplex: Arc<Mutex<DuplexState>>,
}

enum RxStreams {
    Combined(soapysdr::RxStream<Sample>),
    Split(Vec<soapysdr::RxStream<Sample>>),
}

impl RxStreams {
    fn activate(&mut self) -> Result<(), soapysdr::Error> {
        match self {
            Self::Combined(stream) => stream.activate(None),
            Self::Split(streams) => {
                for index in 0..streams.len() {
                    if let Err(error) = streams[index].activate(None) {
                        for active in &mut streams[..index] {
                            let _ = active.deactivate(None);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }
}

impl SoapyDevice {
    fn from_device(device: soapysdr::Device, identity: ProbeIdentity) -> Result<Self, DeviceError> {
        let capabilities = query_capabilities(&device)?;
        let settings = read_settings(&device, &capabilities);
        let duplex = Arc::new(Mutex::new(DuplexState::new(capabilities.duplex)));
        Ok(Self {
            device,
            capabilities,
            settings,
            identity,
            worker: Worker::new(),
            duplex,
        })
    }

    fn read_extra(&self, key: &str) -> Result<String, DeviceError> {
        if key != GAIN_MODE_SETTING {
            return self.device.read_setting(key).map_err(map_err);
        }
        let count = self.device.num_channels(Direction::Rx).map_err(map_err)?;
        (0..count)
            .map(|channel| {
                self.device
                    .gain_mode(Direction::Rx, channel)
                    .map(|value| value.to_string())
                    .map_err(map_err)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join(","))
    }

    fn write_extra(&self, key: &str, value: &str) -> Result<(), DeviceError> {
        if key != GAIN_MODE_SETTING {
            return self.device.write_setting(key, value).map_err(map_err);
        }
        let values: Vec<&str> = value.split(',').collect();
        let count = self.device.num_channels(Direction::Rx).map_err(map_err)?;
        for channel in 0..count {
            let text = values
                .get(channel)
                .copied()
                .or_else(|| values.first().copied())
                .ok_or_else(|| DeviceError::Unsupported("gain_mode needs a value".to_string()))?;
            let enabled = match text.to_ascii_lowercase().as_str() {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => {
                    return Err(DeviceError::Unsupported(format!(
                        "gain_mode: expected boolean, got {text:?}"
                    )));
                }
            };
            self.device
                .set_gain_mode(Direction::Rx, channel, enabled)
                .map_err(map_err)?;
        }
        Ok(())
    }

    fn write_extras(
        &self,
        writes: &[(String, String)],
    ) -> Result<Vec<(String, String)>, DeviceError> {
        let mut originals = Vec::new();
        for (key, value) in writes {
            let original = match self.read_extra(key) {
                Ok(value) => value,
                Err(error) => {
                    self.restore_extras(&originals);
                    return Err(error);
                }
            };
            originals.push((key.clone(), original));
            if let Err(error) = self.write_extra(key, value) {
                self.restore_extras(&originals);
                return Err(error);
            }
            let echoed = match self.read_extra(key) {
                Ok(value) => value,
                Err(error) => {
                    self.restore_extras(&originals);
                    return Err(error);
                }
            };
            let confirmed = if key == GAIN_MODE_SETTING {
                echoed
                    .split(',')
                    .all(|channel| caps::read_back_confirms(value, channel))
            } else {
                caps::read_back_confirms(value, &echoed)
            };
            if !confirmed {
                self.restore_extras(&originals);
                return Err(DeviceError::Unsupported(format!(
                    "extra setting {key}: driver did not apply {value:?} (reads back {echoed:?})"
                )));
            }
        }
        Ok(originals)
    }

    fn restore_extras(&self, originals: &[(String, String)]) {
        for (key, value) in originals.iter().rev() {
            if let Err(error) = self.write_extra(key, value) {
                tracing::warn!(setting = key, "failed to restore Soapy setting: {error}");
            }
        }
    }

    fn rollback_extras(
        &mut self,
        originals: &[(String, String)],
        previous_capabilities: Capabilities,
    ) {
        self.restore_extras(originals);
        self.capabilities = query_capabilities(&self.device).unwrap_or(previous_capabilities);
        self.settings = read_settings(&self.device, &self.capabilities);
    }

    fn apply_rx_settings(&self, delta: &DeviceSettings) -> Result<(), DeviceError> {
        let Some(directional) = &self.capabilities.directional else {
            return Ok(());
        };
        for channel in &directional.rx {
            let index = channel.channel as usize;
            let settings = delta.for_stream(channel.channel, &self.capabilities.per_stream);
            if let Some(frequency) = settings.center_hz {
                self.device
                    .set_frequency(Direction::Rx, index, frequency, ())
                    .map_err(map_err)?;
            }
            if let Some(rate) = settings.sample_rate {
                self.device
                    .set_sample_rate(Direction::Rx, index, rate)
                    .map_err(map_err)?;
            }
            if let Some(ppm) = settings.ppm {
                self.device
                    .set_component_frequency(Direction::Rx, index, "CORR", ppm, ())
                    .map_err(map_err)?;
            }
            for gain in &settings.gains {
                self.device
                    .set_gain_element(Direction::Rx, index, gain.stage.as_str(), gain.value_db)
                    .map_err(map_err)?;
            }
            if let Some(antenna) = &settings.antenna {
                self.device
                    .set_antenna(Direction::Rx, index, antenna.as_str())
                    .map_err(map_err)?;
            }
            if let Some(bandwidth) = settings.bandwidth {
                self.device
                    .set_bandwidth(Direction::Rx, index, bandwidth)
                    .map_err(map_err)?;
            }
        }
        Ok(())
    }
}

impl SdrDevice for SoapyDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, delta: &DeviceSettings) -> Result<(), DeviceError> {
        let writes: Vec<(String, String)> = delta
            .extra
            .iter()
            .map(|extra| {
                Ok((
                    extra.name.clone(),
                    caps::extra_write_value(&self.capabilities.extra, &extra.name, &extra.value)?,
                ))
            })
            .collect::<Result<_, DeviceError>>()?;
        let previous_capabilities = self.capabilities.clone();
        let originals = self.write_extras(&writes)?;
        if !writes.is_empty() {
            match query_capabilities(&self.device) {
                Ok(capabilities) => self.capabilities = capabilities,
                Err(error) => {
                    self.rollback_extras(&originals, previous_capabilities);
                    return Err(error);
                }
            }
        }
        if let Err(error) = caps::validate(delta, &self.capabilities) {
            self.rollback_extras(&originals, previous_capabilities);
            return Err(error);
        }
        if let Err(error) = self.apply_rx_settings(delta) {
            self.rollback_extras(&originals, previous_capabilities);
            return Err(error);
        }
        if let Some(automatic) = caps::automatic_gain_to_reassert(&writes, delta)
            && let Err(error) = self.write_extra(GAIN_MODE_SETTING, automatic)
        {
            self.rollback_extras(&originals, previous_capabilities);
            return Err(error);
        }
        self.settings.merge_from(delta);
        let actual = read_settings(&self.device, &self.capabilities);
        warn_coerced_rate(delta.sample_rate, actual.sample_rate);
        self.settings.merge_from(&actual);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let channels: Vec<usize> = (0..self.capabilities.rx_streams as usize).collect();
        if sinks.len() != channels.len() {
            return Err(DeviceError::Unsupported(format!(
                "this device has {} rx streams, got {} sinks",
                channels.len(),
                sinks.len()
            )));
        }
        lock(&self.duplex).claim(WireDirection::Rx)?;
        let mut streams = match self.device.rx_stream::<Sample>(&channels) {
            Ok(stream) => RxStreams::Combined(stream),
            Err(combined) if channels.len() > 1 => {
                match channels
                    .iter()
                    .map(|channel| self.device.rx_stream::<Sample>(&[*channel]))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(streams) => RxStreams::Split(streams),
                    Err(split) => {
                        lock(&self.duplex).release(WireDirection::Rx);
                        return Err(DeviceError::Io(format!(
                            "soapy multi-channel stream setup failed: combined: {combined}; \
                             split: {split}"
                        )));
                    }
                }
            }
            Err(error) => {
                lock(&self.duplex).release(WireDirection::Rx);
                return Err(map_err(error));
            }
        };
        if let Err(error) = streams.activate() {
            lock(&self.duplex).release(WireDirection::Rx);
            return Err(map_err(error));
        }
        let identity = self.identity.clone();
        let duplex = self.duplex.clone();
        if let Err(error) = self.worker.start("sdrmm-soapy-rx", move |running| {
            match streams {
                RxStreams::Combined(stream) => capture_loop(stream, &identity, running, sinks),
                RxStreams::Split(streams) => {
                    capture_split_loop(streams, &identity, running, sinks);
                }
            }
            lock(&duplex).release(WireDirection::Rx);
        }) {
            lock(&self.duplex).release(WireDirection::Rx);
            return Err(error);
        }
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
        lock(&self.duplex).release(WireDirection::Rx);
    }

    fn tx_start_channels(&mut self, channels: &[u32]) -> Result<Box<dyn TxStream>, DeviceError> {
        if channels.is_empty() {
            return Err(DeviceError::Unsupported(
                "a transmit stream needs at least one channel".to_string(),
            ));
        }
        let available = self.capabilities.tx_streams;
        let mut native = Vec::with_capacity(channels.len());
        for channel in channels {
            if *channel >= available {
                return Err(DeviceError::Unsupported(format!(
                    "tx channel {channel}: this device has {available}"
                )));
            }
            if native.contains(&(*channel as usize)) {
                return Err(DeviceError::Unsupported(format!(
                    "tx channel {channel} was requested twice"
                )));
            }
            native.push(*channel as usize);
        }
        lock(&self.duplex).claim(WireDirection::Tx)?;
        let mut stream = match self.device.tx_stream::<Sample>(&native) {
            Ok(stream) => stream,
            Err(error) => {
                lock(&self.duplex).release(WireDirection::Tx);
                return Err(map_err(error));
            }
        };
        if let Err(error) = stream.activate(None) {
            lock(&self.duplex).release(WireDirection::Tx);
            return Err(map_err(error));
        }
        Ok(Box::new(SoapyTx::new(
            Box::new(stream),
            native.len(),
            self.duplex.clone(),
        )))
    }
}

fn capture_loop(
    mut stream: soapysdr::RxStream<Sample>,
    identity: &ProbeIdentity,
    running: &AtomicBool,
    mut sinks: Vec<RxSink>,
) {
    let block = stream.mtu().unwrap_or(MIN_BLOCK).max(MIN_BLOCK);
    let mut buffers = vec![vec![Sample::new(0.0, 0.0); block]; sinks.len()];
    let mut watchdog = Watchdog::new(Instant::now());
    let mut overflows = 0u64;
    while running.load(Ordering::Acquire) {
        let result = {
            let mut slices: Vec<&mut [Sample]> =
                buffers.iter_mut().map(Vec::as_mut_slice).collect();
            stream.read(&mut slices, READ_TIMEOUT_US)
        };
        match result {
            Ok(count) => {
                watchdog.delivered(Instant::now());
                if count > 0 {
                    for (sink, buffer) in sinks.iter_mut().zip(&buffers) {
                        sink.push(&buffer[..count]);
                    }
                }
            }
            Err(error) if error.code == ErrorCode::Timeout => {
                match watchdog.timed_out(Instant::now()) {
                    Watch::Wait => continue,
                    Watch::Silent => {
                        fail_all(&mut sinks, &silent_stream(watchdog.silence()));
                        break;
                    }
                    Watch::Probe => {}
                }
                match identity.is_present() {
                    Ok(true) => watchdog.present(),
                    Ok(false) => {
                        fail_all_gone(&mut sinks, "it no longer enumerates");
                        break;
                    }
                    Err(probe) => {
                        if watchdog.probe_failed() {
                            fail_all(
                                &mut sinks,
                                &format!("device lost: enumerate failed: {probe}"),
                            );
                            break;
                        }
                    }
                }
            }
            Err(error) if error.code == ErrorCode::Overflow => {
                overflows += 1;
                if overflows == 1 || overflows.is_multiple_of(OVERFLOW_LOG_EVERY) {
                    tracing::warn!(overflows, "soapy rx overflow");
                }
            }
            Err(error) => {
                fail_all(&mut sinks, &format!("stream read failed: {error}"));
                break;
            }
        }
    }
    if let Err(error) = stream.deactivate(None) {
        tracing::debug!("soapy stream deactivate failed: {error}");
    }
}

fn capture_split_loop(
    mut streams: Vec<soapysdr::RxStream<Sample>>,
    identity: &ProbeIdentity,
    running: &AtomicBool,
    mut sinks: Vec<RxSink>,
) {
    let mut buffers: Vec<Vec<Sample>> = streams
        .iter()
        .map(|stream| vec![Sample::new(0.0, 0.0); stream.mtu().unwrap_or(MIN_BLOCK).max(MIN_BLOCK)])
        .collect();
    let mut watchdogs: Vec<Watchdog> = (0..streams.len())
        .map(|_| Watchdog::new(Instant::now()))
        .collect();
    let mut overflows = vec![0u64; streams.len()];
    'capture: while running.load(Ordering::Acquire) {
        for channel in 0..streams.len() {
            let result = streams[channel].read(&mut [&mut buffers[channel]], READ_TIMEOUT_US);
            match result {
                Ok(count) => {
                    watchdogs[channel].delivered(Instant::now());
                    if count > 0 {
                        sinks[channel].push(&buffers[channel][..count]);
                    }
                }
                Err(error) if error.code == ErrorCode::Timeout => {
                    match watchdogs[channel].timed_out(Instant::now()) {
                        Watch::Wait => continue,
                        Watch::Silent => {
                            let silence = watchdogs[channel].silence();
                            fail_all(
                                &mut sinks,
                                &format!("stream {channel}: {}", silent_stream(silence)),
                            );
                            break 'capture;
                        }
                        Watch::Probe => {}
                    }
                    match identity.is_present() {
                        Ok(true) => watchdogs[channel].present(),
                        Ok(false) => {
                            fail_all_gone(&mut sinks, "it no longer enumerates");
                            break 'capture;
                        }
                        Err(probe) => {
                            if watchdogs[channel].probe_failed() {
                                fail_all(
                                    &mut sinks,
                                    &format!("device lost: enumerate failed: {probe}"),
                                );
                                break 'capture;
                            }
                        }
                    }
                }
                Err(error) if error.code == ErrorCode::Overflow => {
                    overflows[channel] += 1;
                    if overflows[channel] == 1
                        || overflows[channel].is_multiple_of(OVERFLOW_LOG_EVERY)
                    {
                        tracing::warn!(
                            channel,
                            overflows = overflows[channel],
                            "soapy rx overflow"
                        );
                    }
                }
                Err(error) => {
                    fail_all(
                        &mut sinks,
                        &format!("stream {channel} read failed: {error}"),
                    );
                    break 'capture;
                }
            }
        }
    }
    for (channel, stream) in streams.iter_mut().enumerate() {
        if let Err(error) = stream.deactivate(None) {
            tracing::debug!(channel, "soapy stream deactivate failed: {error}");
        }
    }
}

fn silent_stream(silence: Duration) -> String {
    format!(
        "the radio stopped sending samples for {silence:?} but is still plugged in — another \
         program may have taken it over, or it needs to be re-plugged"
    )
}

fn fail_all(sinks: &mut [RxSink], message: &str) {
    for sink in sinks {
        sink.fail(DeviceError::Io(message.to_string()));
    }
}

fn fail_all_gone(sinks: &mut [RxSink], reason: &str) {
    for sink in sinks {
        sink.fail(DeviceError::Disconnected(reason.to_string()));
    }
}

trait TxIo: Send {
    fn write(
        &mut self,
        buffers: &[&[Sample]],
        end_burst: bool,
        timeout_us: i64,
    ) -> Result<usize, soapysdr::Error>;
    fn read_status(&mut self, timeout_us: i64) -> Result<TxStatus, soapysdr::Error>;
    fn deactivate(&mut self) -> Result<(), soapysdr::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TxStatus {
    channels: usize,
    flags: i32,
    time_ns: i64,
    elements: usize,
}

impl TxIo for soapysdr::TxStream<Sample> {
    fn write(
        &mut self,
        buffers: &[&[Sample]],
        end_burst: bool,
        timeout_us: i64,
    ) -> Result<usize, soapysdr::Error> {
        self.write(buffers, None, end_burst, timeout_us)
    }

    fn read_status(&mut self, timeout_us: i64) -> Result<TxStatus, soapysdr::Error> {
        let mut channels = 0;
        let mut flags = 0;
        let mut time_ns = 0;
        let elements = self.read_status(&mut channels, &mut flags, &mut time_ns, timeout_us)?;
        Ok(TxStatus {
            channels,
            flags,
            time_ns,
            elements,
        })
    }

    fn deactivate(&mut self) -> Result<(), soapysdr::Error> {
        self.deactivate(None)
    }
}

struct SoapyTx {
    io: Option<Box<dyn TxIo>>,
    channels: usize,
    duplex: Arc<Mutex<DuplexState>>,
}

impl SoapyTx {
    fn new(io: Box<dyn TxIo>, channels: usize, duplex: Arc<Mutex<DuplexState>>) -> Self {
        Self {
            io: Some(io),
            channels,
            duplex,
        }
    }

    fn status(io: &mut dyn TxIo) -> Result<(), DeviceError> {
        match io.read_status(0) {
            Ok(status) => {
                tracing::debug!(
                    channels = status.channels,
                    flags = status.flags,
                    time_ns = status.time_ns,
                    elements = status.elements,
                    "soapy tx status"
                );
                Ok(())
            }
            Err(error) if error.code == ErrorCode::Timeout => Ok(()),
            Err(error) if error.code == ErrorCode::Underflow => {
                tracing::warn!("soapy tx underflow: {error}");
                Err(DeviceError::Io(format!("tx underflow: {error}")))
            }
            Err(error) => Err(DeviceError::Io(format!("tx status failed: {error}"))),
        }
    }
}

impl TxStream for SoapyTx {
    fn write_channels(
        &mut self,
        channels: &[&[Sample]],
        timeout: Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError> {
        if channels.len() != self.channels {
            return Err(DeviceError::Unsupported(format!(
                "this tx stream has {} channels, got {} buffers",
                self.channels,
                channels.len()
            )));
        }
        let length = channels.first().map_or(0, |samples| samples.len());
        if channels.iter().any(|samples| samples.len() != length) {
            return Err(DeviceError::Unsupported(
                "all tx channel buffers must have the same length".to_string(),
            ));
        }
        let io = self
            .io
            .as_deref_mut()
            .ok_or_else(|| DeviceError::Io("tx stream is stopped".to_string()))?;
        let micros = i64::try_from(timeout.as_micros().min(i64::MAX as u128)).unwrap_or(i64::MAX);
        let written = match io.write(channels, end_burst, micros) {
            Ok(written) => written,
            Err(error) if error.code == ErrorCode::Timeout => 0,
            Err(error) => return Err(DeviceError::Io(format!("tx write failed: {error}"))),
        };
        Self::status(io)?;
        Ok(written)
    }

    fn stop(&mut self) -> Result<(), DeviceError> {
        let Some(mut io) = self.io.take() else {
            return Ok(());
        };
        let status = Self::status(io.as_mut());
        let deactivate = io
            .deactivate()
            .map_err(|error| DeviceError::Io(format!("tx deactivate failed: {error}")));
        lock(&self.duplex).release(WireDirection::Tx);
        status.and(deactivate)
    }
}

impl Drop for SoapyTx {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            tracing::warn!("failed to stop Soapy TX stream: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use sdrmm_wire::Duplex;

    use super::*;

    struct MockTx {
        writes: VecDeque<Result<usize, soapysdr::Error>>,
        statuses: VecDeque<Result<TxStatus, soapysdr::Error>>,
        deactivated: Arc<AtomicBool>,
    }

    impl TxIo for MockTx {
        fn write(
            &mut self,
            _buffers: &[&[Sample]],
            _end_burst: bool,
            _timeout_us: i64,
        ) -> Result<usize, soapysdr::Error> {
            self.writes.pop_front().expect("mock write")
        }

        fn read_status(&mut self, _timeout_us: i64) -> Result<TxStatus, soapysdr::Error> {
            self.statuses.pop_front().unwrap_or_else(|| {
                Err(soapysdr::Error {
                    code: ErrorCode::Timeout,
                    message: "none".to_string(),
                })
            })
        }

        fn deactivate(&mut self) -> Result<(), soapysdr::Error> {
            self.deactivated.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn tx(
        writes: Vec<Result<usize, soapysdr::Error>>,
        statuses: Vec<Result<TxStatus, soapysdr::Error>>,
    ) -> (SoapyTx, Arc<AtomicBool>, Arc<Mutex<DuplexState>>) {
        let deactivated = Arc::new(AtomicBool::new(false));
        let duplex = Arc::new(Mutex::new(DuplexState::new(Duplex::Half)));
        lock(&duplex).claim(WireDirection::Tx).unwrap();
        (
            SoapyTx::new(
                Box::new(MockTx {
                    writes: writes.into(),
                    statuses: statuses.into(),
                    deactivated: deactivated.clone(),
                }),
                1,
                duplex.clone(),
            ),
            deactivated,
            duplex,
        )
    }

    fn error(code: ErrorCode) -> soapysdr::Error {
        soapysdr::Error {
            code,
            message: format!("{code:?}"),
        }
    }

    fn failed(message: &str) -> DeviceError {
        map_err(soapysdr::Error {
            code: ErrorCode::Other,
            message: message.to_string(),
        })
    }

    #[test]
    fn a_driver_that_cannot_claim_the_hardware_reads_as_in_use() {
        for message in [
            "SoapySDR::Device::make() failed: Unable to open RTL-SDR device",
            "usb_claim_interface error -6",
            "LIBUSB_ERROR_ACCESS",
            "Device or resource busy",
            "the device is in use",
        ] {
            assert!(
                matches!(failed(message), DeviceError::InUse(_)),
                "{message} must name the radio as taken, not as a plain I/O fault"
            );
        }
    }

    #[test]
    fn any_other_fault_stays_an_io_error() {
        for message in ["stream setup failed", "no such antenna", "timeout"] {
            assert!(
                matches!(failed(message), DeviceError::Io(_)),
                "{message} must not be blamed on another program"
            );
        }
        assert!(matches!(
            map_err(error(ErrorCode::NotSupported)),
            DeviceError::Unsupported(_)
        ));
    }

    #[test]
    fn only_a_substituted_sample_rate_counts_as_coerced() {
        assert!(!rate_was_coerced(2_048_000.0, 2_048_000.0));
        assert!(!rate_was_coerced(2_048_000.0, 2_048_000.001));
        assert!(rate_was_coerced(2_048_000.0, 2_286_826.0));
        assert!(rate_was_coerced(2_400_000.0, 2_048_000.0));
        assert!(!rate_was_coerced(0.0, 2_048_000.0));
        assert!(!rate_was_coerced(2_048_000.0, f64::NAN));
    }

    fn found(args: &str) -> probe::Found {
        let args = soapysdr::Args::from(args);
        probe::Found {
            info: device_info(&args),
            args: args.to_string(),
        }
    }

    #[test]
    fn a_driver_this_build_speaks_to_natively_is_hidden_from_soapy() {
        let driver = SoapyDriver::excluding(["rtlsdr", "hackrf"]);
        assert!(driver.hides(&found("driver=rtlsdr, serial=00000001")));
        assert!(driver.hides(&found("driver=hackrf, serial=675c62dc3b2d4b8b")));
    }

    #[test]
    fn every_other_driver_stays_visible() {
        let driver = SoapyDriver::excluding(["rtlsdr", "hackrf"]);
        for args in [
            "driver=airspy, serial=644064dc2e19a5b",
            "driver=lime, serial=1D3AC4",
            "driver=uhd, serial=31C9245",
            "driver=sdrplay, serial=1809131409",
        ] {
            assert!(!driver.hides(&found(args)), "{args}");
        }
    }

    #[test]
    fn a_driver_that_excludes_nothing_hides_nothing() {
        assert!(!SoapyDriver::new().hides(&found("driver=rtlsdr, serial=00000001")));
    }

    #[test]
    fn a_label_that_merely_names_a_driver_is_not_matched() {
        let driver = SoapyDriver::excluding(["rtlsdr"]);
        assert!(
            !driver.hides(&found("driver=airspy, serial=1, label=not an rtlsdr")),
            "the driver key decides, not a substring of the label"
        );
    }

    #[test]
    fn the_driver_key_is_matched_regardless_of_case() {
        assert!(SoapyDriver::excluding(["rtlsdr"]).hides(&found("driver=RTLSDR, serial=1")));
    }

    #[test]
    fn info_prefers_label_and_serial_key() {
        let args = soapysdr::Args::from("driver=rtlsdr, serial=00000001, label=Generic RTL2832U");
        let info = device_info(&args);
        assert_eq!(info.driver, "soapy");
        assert_eq!(info.key, "00000001");
        assert_eq!(info.label, "Generic RTL2832U");
    }

    #[test]
    fn modes_of_one_serial_have_distinct_keys_and_probe_filters() {
        let single =
            soapysdr::Args::from("driver=example, serial=123456, mode=ST, label=Single Tuner");
        let dual = soapysdr::Args::from("driver=example, serial=123456, mode=DT, label=Dual Tuner");

        assert_eq!(device_info(&single).key, "123456@ST");
        assert_eq!(device_info(&dual).key, "123456@DT");
        assert_eq!(
            ProbeIdentity::from_args(&dual).filter,
            "driver=example,serial=123456,mode=DT"
        );
    }

    #[test]
    fn partial_tx_writes_are_returned_to_the_caller() {
        let (mut tx, _, _) = tx(vec![Ok(3)], vec![Err(error(ErrorCode::Timeout))]);
        let samples = vec![Sample::new(0.0, 0.0); 8];
        assert_eq!(
            tx.write(&samples, Duration::from_millis(10), false)
                .unwrap(),
            3
        );
    }

    #[test]
    fn tx_timeout_is_a_short_zero_write() {
        let (mut tx, _, _) = tx(vec![Err(error(ErrorCode::Timeout))], Vec::new());
        let samples = vec![Sample::new(0.0, 0.0); 8];
        assert_eq!(
            tx.write(&samples, Duration::from_millis(1), false).unwrap(),
            0
        );
    }

    #[test]
    fn tx_underflow_is_reported() {
        let (mut tx, _, _) = tx(vec![Ok(8)], vec![Err(error(ErrorCode::Underflow))]);
        let samples = vec![Sample::new(0.0, 0.0); 8];
        let failure = tx.write(&samples, Duration::from_millis(1), false);
        assert!(matches!(failure, Err(DeviceError::Io(message)) if message.contains("underflow")));
    }

    #[test]
    fn stopping_deactivates_and_releases_duplex_claim() {
        let (mut tx, deactivated, duplex) = tx(Vec::new(), Vec::new());
        tx.stop().unwrap();
        assert!(deactivated.load(Ordering::SeqCst));
        assert!(!lock(&duplex).is_active(WireDirection::Tx));
        lock(&duplex).claim(WireDirection::Rx).unwrap();
    }

    #[test]
    fn multichannel_tx_never_falls_back_to_channel_zero() {
        let deactivated = Arc::new(AtomicBool::new(false));
        let duplex = Arc::new(Mutex::new(DuplexState::new(Duplex::Full)));
        lock(&duplex).claim(WireDirection::Tx).unwrap();
        let mut tx = SoapyTx::new(
            Box::new(MockTx {
                writes: vec![Ok(2)].into(),
                statuses: VecDeque::new(),
                deactivated,
            }),
            2,
            duplex,
        );
        let samples = vec![Sample::new(0.0, 0.0); 2];
        assert!(matches!(
            tx.write(&samples, Duration::ZERO, false),
            Err(DeviceError::Unsupported(_))
        ));
        assert_eq!(
            tx.write_channels(&[&samples, &samples], Duration::ZERO, false)
                .unwrap(),
            2
        );
    }
}
