//! `sdrmm-device-hackrf` — native HackRF backend (PLAN §6, feature `hackrf-native`): pure
//! Rust over `hackrf-nusb`/`nusb`, so release artifacts launch with no libhackrf, no
//! libSoapySDR and no C dependency at all (PLAN §15 packaging rule, milestone M5).
//!
//! What it buys over the Soapy view of the same radio is the real per-stage gain model — LNA
//! and VGA separately, each on its own MAX2837 step grid — plus the RF amplifier and
//! antenna-port bias power as typed extras.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use hackrf_nusb::{DeviceDescriptor, ErrorKind, MaybeFuture, RxStream};
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, Sample, SdrDevice};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod caps;

const DRIVER_ID: &str = "hackrf";
/// Key prefix for a HackRF whose USB descriptor carries no parseable serial. Prefixed so it
/// can never collide with a hex serial, and index-based because there is nothing else left
/// to key on — see [`HackRfDriver::open`].
const NOSERIAL_KEY_PREFIX: &str = "noserial-";

/// Capture block size. Well under the crate's 131072-sample USB transfer, so a block leaves
/// the loop as soon as one completion has been drained, and large enough that the per-block
/// sink call stays off the sample-rate hot path: 1.6 ms of IQ at 20 Msps, 16 ms at 2.
const BLOCK_SAMPLES: usize = 32_768;
/// Total budget for one `read`, which fills the whole block or returns what it already has
/// when this expires. It bounds how long `rx_stop` waits for the capture thread to notice
/// the stop flag; a healthy stream never reaches it.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// Consecutive empty reads (5 s) before a silent radio is declared dead. A streaming HackRF
/// free-runs and cannot go quiet while healthy, and an unplug fails the queued transfers with
/// `DeviceDisconnected` rather than timing them out — so this fires only for a wedged board,
/// which would otherwise hang the capture thread forever with no fault reported.
const STALL_TIMEOUT_READS: u32 = 50;

/// The crate's error taxonomy onto the four `DeviceError`s. `InvalidConfig` joins
/// `Unsupported` because both mean "the hardware will not take this value", which is what
/// the control plane renders. `DeviceDisconnected` is I/O: the engine's fault path treats
/// any capture error as a lost device and M5 re-opens it when it enumerates again.
fn map_err(err: hackrf_nusb::Error) -> DeviceError {
    match err.kind() {
        ErrorKind::NotFound => DeviceError::NotFound(err.to_string()),
        ErrorKind::InvalidConfig | ErrorKind::Unsupported => {
            DeviceError::Unsupported(err.to_string())
        }
        _ => DeviceError::Io(err.to_string()),
    }
}

/// All 32 hex digits, lowercase — byte-for-byte the string HackRF firmware puts in its USB
/// serial descriptor, which is also what SoapyHackRF reports. The registry collapses probe
/// duplicates by serial (PLAN §6), so rendering the *same* string is what makes one radio
/// seen through both backends merge into one entry instead of two.
fn full_serial(serial: u128) -> String {
    format!("{serial:032x}")
}

/// The form users recognise: `hackrf_info` prints the serial as four 32-bit words, and tools
/// that select one (`hackrf_transfer -d`) match a suffix, because the upper 64 bits are zero
/// on shipped hardware. Label only — the key stays [`full_serial`].
fn short_serial(serial: u128) -> String {
    format!("{:016x}", serial as u64)
}

fn device_label(descriptor: &DeviceDescriptor) -> String {
    let name = descriptor
        .product_string
        .as_deref()
        .unwrap_or(descriptor.description);
    match descriptor.serial {
        Some(serial) => format!("{name} {}", short_serial(serial)),
        None => name.to_string(),
    }
}

fn device_info(descriptor: &DeviceDescriptor, index: usize) -> DeviceInfo {
    let serial = descriptor.serial.map(full_serial);
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: serial
            .clone()
            .unwrap_or_else(|| format!("{NOSERIAL_KEY_PREFIX}{index}")),
        label: device_label(descriptor),
        serial,
    }
}

/// The serial a probe key names, or `None` for the index fallback key.
fn key_serial(key: &str) -> Option<u128> {
    u128::from_str_radix(key, 16).ok()
}

/// Driver for HackRF One / Jawbreaker / rad1o over pure-Rust USB.
#[derive(Default)]
pub struct HackRfDriver;

impl HackRfDriver {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl DeviceDriver for HackRfDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        match hackrf_nusb::Device::list().wait() {
            Ok(found) => found
                .iter()
                .enumerate()
                .map(|(index, descriptor)| device_info(descriptor, index))
                .collect(),
            Err(e) => {
                // probe() cannot return errors; a USB enumerate failure must not pass as a
                // silent "no devices".
                tracing::warn!("hackrf enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let device = match key_serial(&info.key) {
            Some(serial) => hackrf_nusb::Device::open_serial(serial).wait(),
            None => {
                // The descriptor had no parseable serial, so "the first visible HackRF" is
                // the only handle the crate offers. Said out loud, because on a two-radio
                // machine it is the difference between the device the user picked and the
                // one the bus enumerated first.
                tracing::warn!(
                    key = %info.key,
                    "hackrf reports no serial; opening the first visible device"
                );
                hackrf_nusb::Device::open().wait()
            }
        }
        .map_err(map_err)?;
        Ok(Box::new(HackRfDevice::new(device)))
    }
}

/// An opened HackRF receiver.
///
/// `hackrf-nusb` hands out an [`RxStream`] that owns its USB endpoint and transfer queue and
/// borrows nothing from the `Device`, so the capture thread holds the stream while `apply`
/// retunes through `&mut self`. The crate serializes the two on a lifecycle mutex the read
/// path never takes, which is what makes a live retune cost nothing on the sample path.
pub struct HackRfDevice {
    device: hackrf_nusb::Device,
    capabilities: Capabilities,
    settings: DeviceSettings,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl HackRfDevice {
    fn new(device: hackrf_nusb::Device) -> Self {
        let settings = caps::settings_from_config(device.config());
        Self {
            device,
            capabilities: caps::capabilities(),
            settings,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    fn write_to_hardware(&mut self, applied: &caps::Applied) -> Result<(), DeviceError> {
        if let Some(hz) = applied.frequency_hz {
            self.device.set_frequency_hz(hz).wait().map_err(map_err)?;
        }
        if let Some(rate) = applied.sample_rate_hz {
            self.device
                .set_sample_rate_hz(rate)
                .wait()
                .map_err(map_err)?;
        }
        if let Some(db) = applied.lna_gain_db {
            self.device.set_lna_gain_db(db).wait().map_err(map_err)?;
        }
        if let Some(db) = applied.vga_gain_db {
            self.device.set_vga_gain_db(db).wait().map_err(map_err)?;
        }
        if let Some(enabled) = applied.amp {
            self.device
                .set_amp_enable(enabled)
                .wait()
                .map_err(map_err)?;
        }
        if let Some(enabled) = applied.bias_tee {
            self.device.set_bias_tee(enabled).wait().map_err(map_err)?;
        }
        Ok(())
    }
}

impl SdrDevice for HackRfDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let applied = caps::validate(settings, &self.capabilities)?;
        let result = self.write_to_hardware(&applied);
        // The crate records a field only once its control transfer succeeded, so rebuilding
        // from `config()` reports exactly what the hardware holds: gains as the step grid
        // snapped them, and — when a batch failed halfway — the prefix that did land, never
        // the values that were asked for.
        self.settings = caps::settings_from_config(self.device.config());
        result
    }

    fn rx_start(&mut self, sink: RxSink) -> Result<(), DeviceError> {
        if self.worker.is_some() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let mut stream = self.device.rx_stream().map_err(map_err)?;
        stream.start().wait().map_err(map_err)?;
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        match std::thread::Builder::new()
            .name("sdrmm-hackrf-rx".to_string())
            .spawn(move || capture_loop(stream, &running, sink))
        {
            Ok(worker) => {
                self.worker = Some(worker);
                Ok(())
            }
            Err(e) => {
                // The un-spawned closure drops the stream, which stops it; clear the flag so
                // a retry is not left looking like a live capture.
                self.running.store(false, Ordering::Release);
                Err(DeviceError::Io(format!("spawn capture thread: {e}")))
            }
        }
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for HackRfDevice {
    fn drop(&mut self) {
        self.rx_stop();
        // Documented lifecycle: the stream must be stopped *and* dropped before shutdown or
        // it answers Busy — the capture thread does both before `rx_stop` returns. Waiting
        // on shutdown here makes the "radio off" transfer observable instead of leaving it
        // to the crate's best-effort drop.
        if let Err(e) = self.device.shutdown().wait() {
            tracing::debug!("hackrf shutdown failed: {e}");
        }
    }
}

/// Blocking read loop on the capture thread. Owns the stream and stops it on every exit path
/// (stop flag, unplug, fatal stream error), which is also what releases the device's RX claim
/// so a later `rx_start` can take it again.
fn capture_loop(mut stream: RxStream, running: &AtomicBool, mut sink: RxSink) {
    let mut buf = vec![Sample::new(0.0, 0.0); BLOCK_SAMPLES];
    let mut idle_reads = 0u32;
    while running.load(Ordering::Acquire) {
        match stream.read(&mut buf, Some(READ_TIMEOUT)).wait() {
            Ok(0) => {
                idle_reads += 1;
                if idle_reads >= STALL_TIMEOUT_READS {
                    sink.fail(DeviceError::Io(format!(
                        "device stalled: no samples for {:?}",
                        READ_TIMEOUT * STALL_TIMEOUT_READS
                    )));
                    break;
                }
            }
            Ok(n) => {
                idle_reads = 0;
                sink.push(&buf[..n]);
            }
            Err(e) if e.kind() == ErrorKind::DeviceDisconnected => {
                sink.fail(DeviceError::Io(format!("device lost: {e}")));
                break;
            }
            Err(e) => {
                // Any read error invalidates the stream — the crate turns the radio off and
                // requires a fresh `RxStream` — so there is nothing to retry here.
                sink.fail(DeviceError::Io(format!("stream read failed: {e}")));
                break;
            }
        }
    }
    match stream.stop().wait() {
        // `buffers_dropped` counts USB transfers that completed with an error, *not*
        // device-side sample overruns: the HackRF does not report those and the crate cannot
        // infer them. A non-zero count means the run ended on a transfer fault, so it is
        // reported rather than swallowed; it is only readable at stop, as the stream exposes
        // no live stats accessor.
        Ok(stats) if stats.buffers_dropped > 0 => tracing::warn!(
            dropped = stats.buffers_dropped,
            received = stats.buffers_received,
            "hackrf rx ended with failed USB transfers"
        ),
        Ok(stats) => tracing::debug!(
            received = stats.buffers_received,
            processed = stats.buffers_processed,
            "hackrf rx stopped"
        ),
        Err(e) => tracing::debug!("hackrf stream stop failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIAL: u128 = 0x0000_0000_0000_0000_675c_62dc_3b2d_4b8b;

    fn descriptor(serial: Option<u128>, product_string: Option<&str>) -> DeviceDescriptor {
        DeviceDescriptor {
            vid: 0x1d50,
            pid: 0x6089,
            description: "HackRF One / HackRF Pro",
            serial,
            product_string: product_string.map(str::to_string),
            usb_api_version: 0x0107,
        }
    }

    #[test]
    fn key_is_the_full_usb_serial_so_the_registry_can_merge() {
        let info = device_info(&descriptor(Some(SERIAL), None), 0);
        assert_eq!(info.driver, "hackrf");
        assert_eq!(info.key, "0000000000000000675c62dc3b2d4b8b");
        assert_eq!(
            info.serial.as_deref(),
            Some("0000000000000000675c62dc3b2d4b8b")
        );
        assert_eq!(info.id(), "hackrf:0000000000000000675c62dc3b2d4b8b");
        // Leading zeroes must survive: the string has to match SoapyHackRF's byte for byte.
        assert_eq!(info.key.len(), 32);
    }

    #[test]
    fn label_shows_the_short_serial_users_recognise() {
        assert_eq!(
            device_label(&descriptor(Some(SERIAL), None)),
            "HackRF One / HackRF Pro 675c62dc3b2d4b8b"
        );
        // A firmware product string wins over the static USB-ID table description.
        assert_eq!(
            device_label(&descriptor(Some(SERIAL), Some("HackRF One"))),
            "HackRF One 675c62dc3b2d4b8b"
        );
    }

    #[test]
    fn short_serial_keeps_the_low_64_bits() {
        assert_eq!(short_serial(SERIAL), "675c62dc3b2d4b8b");
        assert_eq!(short_serial(u128::MAX), "ffffffffffffffff");
        assert_eq!(short_serial(0), "0000000000000000");
    }

    #[test]
    fn serialless_device_gets_an_index_key_and_no_serial() {
        let info = device_info(&descriptor(None, Some("rad1o")), 2);
        assert_eq!(info.key, "noserial-2");
        assert_eq!(info.serial, None);
        assert_eq!(info.label, "rad1o");
        // Never a hex serial, so it cannot be mistaken for one on the way back in.
        assert_eq!(key_serial(&info.key), None);
    }

    #[test]
    fn keys_round_trip_back_to_the_serial_open_needs() {
        let info = device_info(&descriptor(Some(SERIAL), None), 0);
        assert_eq!(key_serial(&info.key), Some(SERIAL));
        assert_eq!(key_serial("675c62dc3b2d4b8b"), Some(0x675c_62dc_3b2d_4b8b));
        assert_eq!(key_serial("not-a-serial"), None);
    }

    #[test]
    fn error_kinds_map_onto_the_right_device_errors() {
        // `hackrf_nusb::Error` cannot be constructed from outside the crate, so the mapping
        // is exercised through the one variant its public API produces: a config value the
        // driver refuses.
        let refused = hackrf_nusb::Config::builder()
            .lna_gain_db(13)
            .build()
            .expect_err("13 dB is off the LNA grid");
        assert_eq!(refused.kind(), ErrorKind::InvalidConfig);
        assert!(matches!(map_err(refused), DeviceError::Unsupported(_)));
    }

    /// The capture split depends on both halves moving across threads: the `SdrDevice` stays
    /// on the control thread while the `RxStream` it produced runs the capture loop. A future
    /// `hackrf-nusb` that tied them together would break that at the `spawn` call site with
    /// no explanation, so the requirement is asserted where the reason is written down.
    #[test]
    fn device_and_stream_cross_thread_boundaries() {
        const fn assert_send<T: Send>() {}
        assert_send::<hackrf_nusb::Device>();
        assert_send::<RxStream>();
        assert_send::<HackRfDevice>();
    }

    // No test may call `HackRfDriver::probe`/`open`: both touch the USB bus, so their result
    // depends on what is plugged into the machine running them (PLAN §14: no hardware in CI,
    // ever). Everything above them is pure and tested here and in `caps`.
}
