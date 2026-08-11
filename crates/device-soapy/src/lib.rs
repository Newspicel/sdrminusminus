//! `sdrmm-device-soapy` — SoapySDR backend (PLAN §6, default feature): one driver covering
//! every device a Soapy module exists for (RTL-SDR, HackRF, Airspy, LimeSDR…). Probe maps
//! Soapy's enumerate args to `DeviceInfo`; open builds `Capabilities` from the channel
//! queries so the client renders controls with zero device-specific frontend code.

use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, DeviceError, RxSink, Sample, SdrDevice, Worker, single_rx_sink};
use sdrmm_wire::{
    Capabilities, DeviceInfo, DeviceSettings, Duplex, GainStage, GainValue, StreamScope,
};
use soapysdr::{Direction, ErrorCode};

mod caps;

const DRIVER_ID: &str = "soapy";
/// RX channel 0 only until multi-channel devices land (PLAN §6 coherent arrays are Phase 4+).
const RX_CHANNEL: usize = 0;

/// Per-read timeout. SoapyRTLSDR reports USB loss only as an endless run of timeouts (no
/// dedicated error code), so ~10 of these in a row triggers a re-enumeration liveness probe.
const READ_TIMEOUT_US: i64 = 100_000;
const UNPLUG_TIMEOUT_READS: u32 = 10;
/// Consecutive enumerate failures the probe tolerates before declaring the device lost —
/// bounded so a broken enumerate path cannot leave a dead stream retrying forever.
const UNPLUG_PROBE_FAILURES: u32 = 2;
/// Minimum spacing between liveness probes while reads keep timing out; enumerate opens the
/// bus, so it must not run per read.
const PROBE_MIN_INTERVAL: Duration = Duration::from_secs(1);
/// Lower bound for the capture buffer when the reported MTU is tiny (or the query fails).
const MIN_BLOCK: usize = 8192;
const OVERFLOW_LOG_EVERY: u64 = 1000;

/// Serializes every enumerate in the process. SoapySDR runs each module's find in a parallel
/// `std::async` per call, and SoapyHackRF's refcounted `hackrf_init`/`hackrf_exit` is not safe
/// against *overlapping* calls: one call can tear down libhackrf's libusb context while
/// another is still inside `hackrf_device_list` (observed twice as SIGSEGV in
/// `libusb_get_device_list`, both during USB re-enumeration after an RTL-SDR brownout). The
/// engine overlaps enumerates by design — hotplug prober, REST device probe, and the capture
/// thread's liveness probe all fire together exactly when USB topology churns — so they must
/// queue here instead.
static ENUMERATE_LOCK: Mutex<()> = Mutex::new(());

fn enumerate_serialized(filter: &str) -> Result<Vec<soapysdr::Args>, soapysdr::Error> {
    let _guard = ENUMERATE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    soapysdr::enumerate(filter)
}

/// Soapy's NotSupported becomes the wire-visible Unsupported; every other code is I/O.
fn map_err(err: soapysdr::Error) -> DeviceError {
    match err.code {
        ErrorCode::NotSupported => DeviceError::Unsupported(err.to_string()),
        _ => DeviceError::Io(err.to_string()),
    }
}

/// Probe key: the serial when present (stable across re-enumeration and USB port moves),
/// else the full args string so the entry is still re-openable while it stays attached.
fn args_key(args: &soapysdr::Args) -> String {
    match args.get("serial") {
        Some(serial) => serial.to_string(),
        None => args.to_string(),
    }
}

fn device_info(args: &soapysdr::Args) -> DeviceInfo {
    let serial = args.get("serial").map(str::to_string);
    let label = match args.get("label") {
        Some(label) => label.to_string(),
        None => {
            let driver = args.get("driver").unwrap_or(DRIVER_ID);
            match &serial {
                Some(serial) => format!("{driver} {serial}"),
                None => driver.to_string(),
            }
        }
    };
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: args_key(args),
        label,
        serial,
        // Soapy has to open the device to answer any of this, and probing must stay cheap: the
        // picker shows an unknown radio as unknown rather than guessing it can run a template.
        profile: None,
    }
}

/// Identity for the capture thread's unplug probe: `filter` narrows re-enumeration to this
/// device's module (and serial when it has one), `key` is what must reappear in the results
/// (see [`args_key`]).
#[derive(Clone, Debug)]
struct ProbeIdentity {
    filter: String,
    key: String,
}

impl ProbeIdentity {
    fn from_args(args: &soapysdr::Args) -> Self {
        let filter = match (args.get("driver"), args.get("serial")) {
            (Some(driver), Some(serial)) => format!("driver={driver},serial={serial}"),
            (Some(driver), None) => format!("driver={driver}"),
            _ => args.to_string(),
        };
        Self {
            filter,
            key: args_key(args),
        }
    }

    /// Whether the device still enumerates. `Err` is an enumerate failure, not absence.
    fn is_present(&self) -> Result<bool, soapysdr::Error> {
        Ok(enumerate_serialized(self.filter.as_str())?
            .iter()
            .any(|args| args_key(args) == self.key))
    }
}

/// Driver that exposes everything SoapySDR enumerates.
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
            Err(e) => {
                // probe() cannot return errors; an enumerate failure must not pass as a
                // silent "no devices".
                tracing::warn!("soapy enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let found = enumerate_serialized("")
            .map_err(|e| DeviceError::Io(format!("soapy enumerate: {e}")))?;
        let args = found
            .into_iter()
            .find(|a| args_key(a) == info.key)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
        // The Soapy module name ("rtlsdr", "hackrf"…) keys the extra-settings table; the
        // binding has no getSettingInfo to query instead.
        let soapy_driver = args.get("driver").unwrap_or_default().to_string();
        let identity = ProbeIdentity::from_args(&args);
        let device = soapysdr::Device::new(args).map_err(map_err)?;
        Ok(Box::new(SoapyDevice::from_device(
            device,
            &soapy_driver,
            identity,
        )?))
    }
}

/// An opened Soapy receiver. The capture thread owns the stream while Soapy's API contract
/// makes device calls thread-safe, so `apply` retunes live without touching the stream.
pub struct SoapyDevice {
    device: soapysdr::Device,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// Whether `list_frequencies` exposes "CORR" — the binding has no setFrequencyCorrection
    /// wrapper, so PPM goes through that component or must error (never dropped).
    ppm_supported: bool,
    identity: ProbeIdentity,
    worker: Worker,
}

/// Overwrite `settings` with the hardware's current state, field by field; a failed query
/// leaves that field untouched. Used at open (fields start unset) and to resync after a
/// failed `apply` batch, where the device may already be gone — hence best effort.
fn read_settings_from_device(
    device: &soapysdr::Device,
    stages: &[GainStage],
    settings: &mut DeviceSettings,
) {
    let dir = Direction::Rx;
    if let Ok(f) = device.frequency(dir, RX_CHANNEL) {
        settings.center_hz = Some(f);
    }
    if let Ok(rate) = device.sample_rate(dir, RX_CHANNEL) {
        settings.sample_rate = Some(rate);
    }
    if let Ok(antenna) = device.antenna(dir, RX_CHANNEL) {
        settings.antenna = Some(antenna);
    }
    // Soapy reports "automatic filter" as 0 Hz; that is not a real bandwidth value.
    if let Ok(bw) = device.bandwidth(dir, RX_CHANNEL) {
        settings.bandwidth = (bw > 0.0).then_some(bw);
    }
    for stage in stages {
        if let Ok(value_db) = device.gain_element(dir, RX_CHANNEL, stage.name.as_str()) {
            match settings.gains.iter_mut().find(|g| g.stage == stage.name) {
                Some(existing) => existing.value_db = value_db,
                None => settings.gains.push(GainValue {
                    stage: stage.name.clone(),
                    value_db,
                }),
            }
        }
    }
}

impl SoapyDevice {
    fn from_device(
        device: soapysdr::Device,
        soapy_driver: &str,
        identity: ProbeIdentity,
    ) -> Result<Self, DeviceError> {
        let dir = Direction::Rx;
        let freq_ranges = device.frequency_range(dir, RX_CHANNEL).map_err(map_err)?;
        let rate_ranges = device
            .get_sample_rate_range(dir, RX_CHANNEL)
            .map_err(map_err)?;
        let (sample_rates, sample_rate_range) = caps::rate_capabilities(&rate_ranges);
        let mut gains = Vec::new();
        for name in device.list_gains(dir, RX_CHANNEL).map_err(map_err)? {
            let range = device
                .gain_element_range(dir, RX_CHANNEL, name.as_str())
                .map_err(map_err)?;
            gains.push(GainStage {
                name,
                range: sdrmm_wire::Range {
                    min: range.minimum,
                    max: range.maximum,
                    step: (range.step > 0.0).then_some(range.step),
                },
            });
        }
        let bw_ranges = device.bandwidth_range(dir, RX_CHANNEL).map_err(map_err)?;
        // Probed before the capabilities are built, so the flag the client renders from and the
        // check `apply` refuses on are the same value — a tuner that advertised a correction it
        // then rejected would be the exact bug this capability exists to remove.
        let ppm_supported = device
            .list_frequencies(dir, RX_CHANNEL)
            .map(|components| components.iter().any(|c| c == "CORR"))
            .unwrap_or(false);
        let capabilities = Capabilities {
            freq_ranges: caps::freq_ranges(&freq_ranges),
            sample_rates,
            sample_rate_range,
            gains,
            antennas: device.antennas(dir, RX_CHANNEL).map_err(map_err)?,
            bandwidths: caps::discrete_points(&bw_ranges),
            extra: caps::extra_settings(soapy_driver),
            ppm: ppm_supported,
            duplex: Duplex::RxOnly,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
        };

        // Settings start as the hardware's current state so clients render reality, not a
        // guess. Query failures leave the field unset rather than failing the open.
        let mut settings = DeviceSettings::default();
        read_settings_from_device(&device, &capabilities.gains, &mut settings);

        Ok(Self {
            device,
            capabilities,
            settings,
            ppm_supported,
            identity,
            worker: Worker::new(),
        })
    }

    fn apply_to_hardware(
        &self,
        settings: &DeviceSettings,
        extra_writes: &[(String, String)],
    ) -> Result<(), DeviceError> {
        let dir = Direction::Rx;
        if let Some(f) = settings.center_hz {
            self.device
                .set_frequency(dir, RX_CHANNEL, f, ())
                .map_err(map_err)?;
        }
        if let Some(rate) = settings.sample_rate {
            self.device
                .set_sample_rate(dir, RX_CHANNEL, rate)
                .map_err(map_err)?;
        }
        if let Some(ppm) = settings.ppm {
            self.device
                .set_component_frequency(dir, RX_CHANNEL, "CORR", ppm, ())
                .map_err(map_err)?;
        }
        for gain in &settings.gains {
            self.device
                .set_gain_element(dir, RX_CHANNEL, gain.stage.as_str(), gain.value_db)
                .map_err(map_err)?;
        }
        if let Some(antenna) = &settings.antenna {
            self.device
                .set_antenna(dir, RX_CHANNEL, antenna.as_str())
                .map_err(map_err)?;
        }
        if let Some(bandwidth) = settings.bandwidth {
            self.device
                .set_bandwidth(dir, RX_CHANNEL, bandwidth)
                .map_err(map_err)?;
        }
        for (key, value) in extra_writes {
            self.device
                .write_setting(key.as_str(), value.as_str())
                .map_err(map_err)?;
            // SoapyRTLSDR's writeSetting returns success for keys it ignores (e.g. biastee
            // compiled out against an old librtlsdr), so only a read-back proves the value
            // took effect.
            let echoed = self.device.read_setting(key.as_str()).map_err(map_err)?;
            if !caps::read_back_confirms(value, &echoed) {
                return Err(DeviceError::Unsupported(format!(
                    "extra setting {key}: driver did not apply {value:?} (reads back {echoed:?})"
                )));
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

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let extra_writes = caps::validate(settings, &self.capabilities, self.ppm_supported)?;
        if let Err(e) = self.apply_to_hardware(settings, &extra_writes) {
            // A mid-batch failure leaves the hardware partially retuned; resync recorded
            // settings to the device's actual state so the control plane (and the engine's
            // spectrum metadata) never keep reporting pre-batch values the device dropped.
            read_settings_from_device(&self.device, &self.capabilities.gains, &mut self.settings);
            return Err(e);
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let sink = single_rx_sink(sinks)?;
        if self.worker.is_running() {
            return Err(DeviceError::AlreadyStreaming);
        }
        // Activated here rather than on the worker so a stream the driver refuses reports
        // through this `Result` instead of through the engine's fault path a moment later.
        let mut stream = self
            .device
            .rx_stream::<Sample>(&[RX_CHANNEL])
            .map_err(map_err)?;
        stream.activate(None).map_err(map_err)?;
        let identity = self.identity.clone();
        self.worker.start("sdrmm-soapy-rx", move |running| {
            capture_loop(stream, &identity, running, sink);
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

/// Blocking read loop on the capture thread. Owns the stream; deactivates it on every exit
/// path (stop flag, unplug, fatal stream error).
fn capture_loop(
    mut stream: soapysdr::RxStream<Sample>,
    identity: &ProbeIdentity,
    running: &AtomicBool,
    mut sink: RxSink,
) {
    let block = stream.mtu().unwrap_or(MIN_BLOCK).max(MIN_BLOCK);
    let mut buf = vec![Sample::new(0.0, 0.0); block];
    let mut timeouts = 0u32;
    let mut probe_failures = 0u32;
    let mut last_probe: Option<Instant> = None;
    let mut overflows = 0u64;
    while running.load(Ordering::Acquire) {
        match stream.read(&mut [&mut buf], READ_TIMEOUT_US) {
            Ok(n) => {
                timeouts = 0;
                probe_failures = 0;
                if n > 0 {
                    sink.push(&buf[..n]);
                }
            }
            Err(e) if e.code == ErrorCode::Timeout => {
                timeouts += 1;
                if timeouts < UNPLUG_TIMEOUT_READS
                    || last_probe.is_some_and(|at| at.elapsed() < PROBE_MIN_INTERVAL)
                {
                    continue;
                }
                // Persistent timeouts: tell a quiet stream apart from a vanished device.
                // hardware_key() cannot do this — SoapyRTLSDR caches it at open (zero USB
                // I/O) and SoapyHackRF swallows its board-read failure, so it keeps
                // succeeding after a physical unplug. Only re-enumeration touches the bus.
                last_probe = Some(Instant::now());
                match identity.is_present() {
                    Ok(true) => {
                        timeouts = 0;
                        probe_failures = 0;
                    }
                    Ok(false) => {
                        sink.fail(DeviceError::Io(
                            "device lost: no longer enumerates".to_string(),
                        ));
                        break;
                    }
                    Err(probe) => {
                        probe_failures += 1;
                        if probe_failures >= UNPLUG_PROBE_FAILURES {
                            sink.fail(DeviceError::Io(format!(
                                "device lost: enumerate failed: {probe}"
                            )));
                            break;
                        }
                    }
                }
            }
            Err(e) if e.code == ErrorCode::Overflow => {
                // Dropped samples, not a dead device: count, log throttled, keep reading.
                overflows += 1;
                if overflows == 1 || overflows.is_multiple_of(OVERFLOW_LOG_EVERY) {
                    tracing::warn!(overflows, "soapy rx overflow");
                }
            }
            Err(e) => {
                sink.fail(DeviceError::Io(format!("stream read failed: {e}")));
                break;
            }
        }
    }
    if let Err(e) = stream.deactivate(None) {
        tracing::debug!("soapy stream deactivate failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_prefers_label_and_serial_key() {
        let args = soapysdr::Args::from("driver=rtlsdr, serial=00000001, label=Generic RTL2832U");
        let info = device_info(&args);
        assert_eq!(info.driver, "soapy");
        assert_eq!(info.key, "00000001");
        assert_eq!(info.label, "Generic RTL2832U");
        assert_eq!(info.serial.as_deref(), Some("00000001"));
        assert_eq!(info.id(), "soapy:00000001");
    }

    #[test]
    fn info_without_serial_uses_args_string_key() {
        let args = soapysdr::Args::from("driver=audio, device_id=0");
        let info = device_info(&args);
        assert_eq!(info.serial, None);
        assert_eq!(info.key, args.to_string());
        assert_eq!(info.label, "audio");
    }

    #[test]
    fn info_composes_label_from_driver_and_serial() {
        let args = soapysdr::Args::from("driver=hackrf, serial=0123");
        let info = device_info(&args);
        assert_eq!(info.label, "hackrf 0123");
    }

    #[test]
    fn probe_identity_filters_by_driver_and_serial() {
        let args = soapysdr::Args::from("driver=rtlsdr, serial=00000001, label=Generic RTL2832U");
        let identity = ProbeIdentity::from_args(&args);
        assert_eq!(identity.filter, "driver=rtlsdr,serial=00000001");
        assert_eq!(identity.key, "00000001");
    }

    #[test]
    fn probe_identity_without_serial_matches_on_args_string() {
        let args = soapysdr::Args::from("driver=audio, device_id=0");
        let identity = ProbeIdentity::from_args(&args);
        assert_eq!(identity.filter, "driver=audio");
        assert_eq!(identity.key, args.to_string());
    }

    // No test may call `SoapyDriver::probe`/`open`: live enumerate loads whatever Soapy
    // modules the machine has (SoapyUHD aborts the process headless, SoapyRemote scans the
    // network) — environment-dependent by construction (PLAN §14: no hardware in CI, ever).
}
