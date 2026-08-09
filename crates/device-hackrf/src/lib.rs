//! `sdrmm-device-hackrf` — native HackRF backend (PLAN §6, feature `hackrf-native`): pure Rust
//! over `nusb` via the vendored `sdrmm-hackrf-driver`, so release artifacts launch with no
//! libhackrf, no libSoapySDR and no C dependency at all (PLAN §15 packaging rule, milestone M5).
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
};

use convert::IqConverter;
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, SdrDevice};
use sdrmm_hackrf_driver::{DeviceDescriptor, RX_TRANSFER_SIZE};
use sdrmm_usb_stream::{RxStream, Stopper};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod caps;
mod convert;

const DRIVER_ID: &str = "hackrf";
/// Key prefix for a HackRF whose USB descriptor carries no parseable serial. Prefixed so it
/// can never collide with a hex serial, and index-based because there is nothing else left
/// to key on — see [`HackRfDriver::open`].
const NOSERIAL_KEY_PREFIX: &str = "noserial-";

/// Samples per push into the engine's ring. One USB transfer is 128 Ki samples — 65 ms at
/// 2 Msps — which is a coarse unit for a ring the DSP thread drains continuously, so a block is
/// split before it goes downstream. Small enough to keep latency low, large enough that the
/// per-block indirect call stays off the sample-rate hot path.
const BLOCK_SAMPLES: usize = 32_768;

/// The driver's error taxonomy onto the four `DeviceError`s. `InvalidConfig` becomes
/// `Unsupported` because it means "the hardware will not take this value", which is what the
/// control plane renders. Everything USB-level is I/O: the engine's fault path treats any
/// capture error as a lost device and re-opens it when it enumerates again.
fn map_err(err: sdrmm_hackrf_driver::Error) -> DeviceError {
    let text = err.to_string();
    match err {
        sdrmm_hackrf_driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        sdrmm_hackrf_driver::Error::InvalidConfig { .. } => DeviceError::Unsupported(text),
        sdrmm_hackrf_driver::Error::AlreadyStreaming => DeviceError::AlreadyStreaming,
        _ => DeviceError::Io(text),
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
        match sdrmm_hackrf_driver::Device::list() {
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
            Some(serial) => sdrmm_hackrf_driver::Device::open_serial(serial),
            None => {
                // The descriptor had no parseable serial, so "the first visible HackRF" is
                // the only handle the driver offers. Said out loud, because on a two-radio
                // machine it is the difference between the device the user picked and the
                // one the bus enumerated first.
                tracing::warn!(
                    key = %info.key,
                    "hackrf reports no serial; opening the first visible device"
                );
                sdrmm_hackrf_driver::Device::open()
            }
        }
        .map_err(map_err)?;
        Ok(Box::new(HackRfDevice::new(device)))
    }
}

/// An opened HackRF receiver.
///
/// The capture thread owns the transport's [`RxStream`], which holds its own bulk endpoint and
/// borrows nothing from the device, so `apply` retunes through the control endpoint while
/// samples keep flowing.
pub struct HackRfDevice {
    device: sdrmm_hackrf_driver::Device,
    capabilities: Capabilities,
    settings: DeviceSettings,
    running: Arc<AtomicBool>,
    /// Live only while streaming: the one way to stop the transfer pump once the stream has
    /// moved to the capture thread.
    stopper: Option<Stopper>,
    worker: Option<JoinHandle<()>>,
}

impl HackRfDevice {
    fn new(device: sdrmm_hackrf_driver::Device) -> Self {
        let settings = caps::settings_from_config(device.config());
        Self {
            device,
            capabilities: caps::capabilities(),
            settings,
            running: Arc::new(AtomicBool::new(false)),
            stopper: None,
            worker: None,
        }
    }

    fn write_to_hardware(&mut self, applied: &caps::Applied) -> Result<(), DeviceError> {
        if let Some(hz) = applied.frequency_hz {
            self.device.set_frequency_hz(hz).map_err(map_err)?;
        }
        if let Some(rate) = applied.sample_rate_hz {
            self.device.set_sample_rate_hz(rate).map_err(map_err)?;
        }
        if let Some(db) = applied.lna_gain_db {
            self.device.set_lna_gain_db(db).map_err(map_err)?;
        }
        if let Some(db) = applied.vga_gain_db {
            self.device.set_vga_gain_db(db).map_err(map_err)?;
        }
        if let Some(enabled) = applied.amp {
            self.device.set_amp_enable(enabled).map_err(map_err)?;
        }
        if let Some(enabled) = applied.bias_tee {
            self.device.set_bias_tee(enabled).map_err(map_err)?;
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
        // The driver records a field only once its control transfer succeeded, so rebuilding
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
        let stream = self.device.start_rx().map_err(map_err)?;
        let stopper = stream.stopper();
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        match std::thread::Builder::new()
            .name("sdrmm-hackrf-rx".to_string())
            .spawn(move || capture_loop(&stream, &running, sink))
        {
            Ok(worker) => {
                self.stopper = Some(stopper);
                self.worker = Some(worker);
                Ok(())
            }
            Err(e) => {
                // The un-spawned closure drops the stream, which releases the endpoint; turn
                // the radio back off and clear the flag so a retry is not left looking like a
                // live capture.
                self.running.store(false, Ordering::Release);
                if let Err(stop) = self.device.stop_rx() {
                    tracing::debug!("hackrf stop after failed spawn: {stop}");
                }
                Err(DeviceError::Io(format!("spawn capture thread: {e}")))
            }
        }
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
        // Radio off first, then the queue: the front end must stop filling transfers that are
        // about to be cancelled.
        if let Err(e) = self.device.stop_rx() {
            tracing::debug!("hackrf stop_rx failed: {e}");
        }
        if let Some(stopper) = self.stopper.take() {
            stopper.stop();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for HackRfDevice {
    fn drop(&mut self) {
        self.rx_stop();
    }
}

/// Blocking capture loop on the capture thread. Borrows the transport stream, which is dropped
/// on every exit path, releasing the device's bulk endpoint so a later `rx_start` can take it.
fn capture_loop(stream: &RxStream, running: &AtomicBool, mut sink: RxSink) {
    let mut converter = IqConverter::with_capacity(RX_TRANSFER_SIZE / 2);
    let mut dropped = 0u64;
    while running.load(Ordering::Acquire) {
        let Some(block) = stream.recv() else {
            // The pump closes the channel when its error policy gives up on the endpoint or
            // when it was told to stop. `running` still set means nobody asked, so it faulted.
            if running.load(Ordering::Acquire) {
                let reason = stream.error().map_or_else(
                    || "usb stream ended".to_string(),
                    std::string::ToString::to_string,
                );
                sink.fail(DeviceError::Io(format!("device lost: {reason}")));
            }
            break;
        };
        for chunk in converter.convert(&block).chunks(BLOCK_SAMPLES) {
            sink.push(chunk);
        }
        // A dropped transfer is a gap in the sample stream that no counter downstream can see,
        // and is a different failure from the engine's ring overruns.
        let total = stream.stats().dropped;
        if total > dropped {
            tracing::warn!(dropped = total, "hackrf dropped usb transfers");
            dropped = total;
        }
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
        assert!(matches!(
            map_err(sdrmm_hackrf_driver::Error::DeviceNotFound),
            DeviceError::NotFound(_)
        ));
        // A value off the MAX2837 grid is "the hardware will not take this", not an I/O fault.
        assert!(matches!(
            map_err(sdrmm_hackrf_driver::Error::InvalidConfig {
                field: "lna_gain_db",
                reason: "must be 0 through 40 dB in 8 dB steps",
            }),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(
            map_err(sdrmm_hackrf_driver::Error::AlreadyStreaming),
            DeviceError::AlreadyStreaming
        ));
        assert!(matches!(
            map_err(sdrmm_hackrf_driver::Error::ControlTransfer(
                nusb::transfer::TransferError::Disconnected
            )),
            DeviceError::Io(_)
        ));
    }

    /// The capture split depends on both halves moving across threads: the `SdrDevice` stays
    /// on the control thread while the `RxStream` it produced runs the capture loop. A change
    /// that tied them together would break that at the `spawn` call site with no explanation,
    /// so the requirement is asserted where the reason is written down.
    #[test]
    fn device_and_stream_cross_thread_boundaries() {
        const fn assert_send<T: Send>() {}
        assert_send::<sdrmm_hackrf_driver::Device>();
        assert_send::<RxStream>();
        assert_send::<HackRfDevice>();
    }

    // No test may call `HackRfDriver::probe`/`open`: both touch the USB bus, so their result
    // depends on what is plugged into the machine running them (PLAN §14: no hardware in CI,
    // ever). Everything above them is pure and tested here and in `caps`.
}
