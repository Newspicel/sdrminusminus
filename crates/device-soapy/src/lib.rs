use std::{
    collections::BTreeMap,
    path::Path,
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

const DRIVER_ID: &str = "soapy";
const READ_TIMEOUT_US: i64 = 100_000;
const UNPLUG_TIMEOUT_READS: u32 = 10;
const UNPLUG_PROBE_FAILURES: u32 = 2;
const PROBE_MIN_INTERVAL: Duration = Duration::from_secs(1);
const MIN_BLOCK: usize = 8192;
const OVERFLOW_LOG_EVERY: u64 = 1000;
const GAIN_MODE_SETTING: &str = "gain_mode";

static ENUMERATE_LOCK: Mutex<()> = Mutex::new(());

/// Details from the loaded SoapySDR core, used by `sdrmm --doctor` and package smoke tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub core_version: String,
    pub search_paths: Vec<String>,
    pub modules: Vec<String>,
}

#[must_use]
pub fn runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        core_version: soapysdr::library_version(),
        search_paths: soapysdr::module_search_paths(),
        modules: soapysdr::list_modules(),
    }
}

/// Select the application's private Soapy tree before the first enumerate or device open.
///
/// # Safety
/// The caller must invoke this during single-threaded process startup, before any other thread
/// can read or write the process environment and before constructing a [`SoapyDriver`].
pub unsafe fn configure_bundled_runtime(root: &Path, modules: &Path) -> Result<(), DeviceError> {
    if !modules.is_dir() {
        return Err(DeviceError::Io(format!(
            "bundled Soapy module directory is missing: {}",
            modules.display()
        )));
    }
    unsafe { std::env::set_var("SOAPY_SDR_ROOT", root) };
    unsafe { std::env::set_var("SOAPY_SDR_PLUGIN_PATH", modules) };
    Ok(())
}

fn enumerate_serialized(filter: &str) -> Result<Vec<soapysdr::Args>, soapysdr::Error> {
    let _guard = ENUMERATE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    soapysdr::enumerate(filter)
}

fn map_err(error: soapysdr::Error) -> DeviceError {
    match error.code {
        ErrorCode::NotSupported => DeviceError::Unsupported(error.to_string()),
        _ => DeviceError::Io(error.to_string()),
    }
}

fn args_key(args: &soapysdr::Args) -> String {
    args.get("serial").map_or_else(
        || args.to_string(),
        |serial| match args.get("mode") {
            // SoapySDRPlay3 enumerates each RSPduo operating mode as a separate device with the
            // same serial. The mode is part of the address upstream too (`serial@mode` in its
            // claimed-device cache), so keep it in our probe key or every choice opens the first
            // mode returned by the module.
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

    fn is_present(&self) -> Result<bool, soapysdr::Error> {
        Ok(enumerate_serialized(&self.filter)?
            .iter()
            .any(|args| args_key(args) == self.key))
    }
}

#[derive(Default)]
pub struct SoapyDriver;

impl SoapyDriver {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl DeviceDriver for SoapyDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        match enumerate_serialized("") {
            Ok(found) => found.iter().map(device_info).collect(),
            Err(error) => {
                tracing::warn!("soapy enumerate failed: {error}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let found = enumerate_serialized("")
            .map_err(|error| DeviceError::Io(format!("soapy enumerate: {error}")))?;
        let args = found
            .into_iter()
            .find(|args| args_key(args) == info.key)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
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

pub struct SoapyDevice {
    device: soapysdr::Device,
    capabilities: Capabilities,
    settings: DeviceSettings,
    identity: ProbeIdentity,
    worker: Worker,
    duplex: Arc<Mutex<DuplexState>>,
}

/// Most Soapy modules accept one stream containing every requested channel. SoapySDRPlay3 is
/// deliberately different for the RSPduo: it exposes two channels but requires one stream handle
/// per channel. Keep the combined path where it exists and fall back to split handles when the
/// module refuses it.
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
        self.settings.merge_from(delta);
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
    let mut timeouts = 0u32;
    let mut probe_failures = 0u32;
    let mut last_probe: Option<Instant> = None;
    let mut overflows = 0u64;
    while running.load(Ordering::Acquire) {
        let result = {
            let mut slices: Vec<&mut [Sample]> =
                buffers.iter_mut().map(Vec::as_mut_slice).collect();
            stream.read(&mut slices, READ_TIMEOUT_US)
        };
        match result {
            Ok(count) => {
                timeouts = 0;
                probe_failures = 0;
                if count > 0 {
                    for (sink, buffer) in sinks.iter_mut().zip(&buffers) {
                        sink.push(&buffer[..count]);
                    }
                }
            }
            Err(error) if error.code == ErrorCode::Timeout => {
                timeouts += 1;
                if timeouts < UNPLUG_TIMEOUT_READS
                    || last_probe.is_some_and(|at| at.elapsed() < PROBE_MIN_INTERVAL)
                {
                    continue;
                }
                last_probe = Some(Instant::now());
                match identity.is_present() {
                    Ok(true) => {
                        timeouts = 0;
                        probe_failures = 0;
                    }
                    Ok(false) => {
                        fail_all(&mut sinks, "device lost: no longer enumerates");
                        break;
                    }
                    Err(probe) => {
                        probe_failures += 1;
                        if probe_failures >= UNPLUG_PROBE_FAILURES {
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
    let mut timeouts = vec![0u32; streams.len()];
    let mut probe_failures = 0u32;
    let mut last_probe: Option<Instant> = None;
    let mut overflows = vec![0u64; streams.len()];
    'capture: while running.load(Ordering::Acquire) {
        for channel in 0..streams.len() {
            let result = streams[channel].read(&mut [&mut buffers[channel]], READ_TIMEOUT_US);
            match result {
                Ok(count) => {
                    timeouts[channel] = 0;
                    probe_failures = 0;
                    if count > 0 {
                        sinks[channel].push(&buffers[channel][..count]);
                    }
                }
                Err(error) if error.code == ErrorCode::Timeout => {
                    timeouts[channel] += 1;
                    if timeouts[channel] < UNPLUG_TIMEOUT_READS
                        || last_probe.is_some_and(|at| at.elapsed() < PROBE_MIN_INTERVAL)
                    {
                        continue;
                    }
                    last_probe = Some(Instant::now());
                    match identity.is_present() {
                        Ok(true) => {
                            timeouts[channel] = 0;
                            probe_failures = 0;
                        }
                        Ok(false) => {
                            fail_all(&mut sinks, "device lost: no longer enumerates");
                            break 'capture;
                        }
                        Err(probe) => {
                            probe_failures += 1;
                            if probe_failures >= UNPLUG_PROBE_FAILURES {
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

fn fail_all(sinks: &mut [RxSink], message: &str) {
    for sink in sinks {
        sink.fail(DeviceError::Io(message.to_string()));
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

    #[test]
    fn info_prefers_label_and_serial_key() {
        let args = soapysdr::Args::from("driver=rtlsdr, serial=00000001, label=Generic RTL2832U");
        let info = device_info(&args);
        assert_eq!(info.driver, "soapy");
        assert_eq!(info.key, "00000001");
        assert_eq!(info.label, "Generic RTL2832U");
    }

    #[test]
    fn sdrplay_duo_modes_have_distinct_keys_and_probe_filters() {
        let single = soapysdr::Args::from(
            "driver=sdrplay, serial=123456, mode=ST, label=RSPduo Single Tuner",
        );
        let dual =
            soapysdr::Args::from("driver=sdrplay, serial=123456, mode=DT, label=RSPduo Dual Tuner");

        assert_eq!(device_info(&single).key, "123456@ST");
        assert_eq!(device_info(&dual).key, "123456@DT");
        assert_eq!(
            ProbeIdentity::from_args(&dual).filter,
            "driver=sdrplay,serial=123456,mode=DT"
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
