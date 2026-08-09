//! `sdrmm-device-hackrf` — native HackRF backend (PLAN §6, feature `hackrf-native`): the HackRF
//! driver and the `SdrDevice` implementation over it, pure Rust on `nusb`, so release artifacts
//! launch with no libhackrf, no libSoapySDR and no C dependency at all (PLAN §15).
//!
//! What it buys over the Soapy view of the same radio is the real per-stage gain model — LNA
//! and VGA separately, each on its own MAX2837 step grid — plus the RF amplifier and
//! antenna-port bias power as typed extras.
//!
//! Four layers, in dependency order:
//!
//! - [`driver`] — the radio: enumeration, the vendor control protocol, RX lifecycle. No wire
//!   types.
//! - `convert` — the HackRF's signed 8-bit IQ to the one `cf32` format the pipeline speaks.
//! - `caps` — the pure translation to the wire capability model, and `apply`'s validation.
//! - this module — `DeviceDriver`/`SdrDevice`, the capture thread and its tier-1 supervisor.
//!
//! Streaming lives in `sdrmm-usb-stream`, shared with the RTL-SDR backend: the transfer queue
//! and the USB error policy are the one thing both radios genuinely have in common, and getting
//! that policy wrong on each side separately is the defect this driver exists to fix (PLAN §17).

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use convert::IqConverter;
use driver::{DeviceDescriptor, HackRf, RX_TRANSFER_SIZE};
use sdrmm_device::{
    DeviceDriver, DeviceError, Recovery, RestartPolicy, RxSink, SILENT_STREAM_TIMEOUT, SdrDevice,
};
use sdrmm_usb_stream::{RxStream, Stopper};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod caps;
mod convert;
mod driver;

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
/// How often the capture loop wakes to re-check its stop flag and the silence clock.
const RECV_POLL: Duration = Duration::from_millis(100);

/// A mutex this crate holds is only ever held across control transfers, so a poisoned one
/// carries no half-written state worth refusing — and refusing would mean losing the radio.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The driver's error taxonomy onto the four `DeviceError`s. `InvalidConfig` becomes
/// `Unsupported` because it means "the hardware will not take this value", which is what the
/// control plane renders. Everything USB-level is I/O: the engine's fault path treats any
/// capture error as a lost device and re-opens it when it enumerates again.
fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
    match err {
        driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        driver::Error::InvalidConfig { .. } => DeviceError::Unsupported(text),
        driver::Error::AlreadyStreaming => DeviceError::AlreadyStreaming,
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
        match HackRf::list() {
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
            Some(serial) => HackRf::open_serial(serial),
            None => {
                // The descriptor had no parseable serial, so "the first visible HackRF" is
                // the only handle the driver offers. Said out loud, because on a two-radio
                // machine it is the difference between the device the user picked and the
                // one the bus enumerated first.
                tracing::warn!(
                    key = %info.key,
                    "hackrf reports no serial; opening the first visible device"
                );
                HackRf::open()
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
///
/// The radio is behind a mutex because the capture thread needs it too: an in-place stream
/// restart (tier 1, `PLAN-NATIVE-DRIVERS.md` §2.2) cycles the transceiver mode from there. The
/// lock is never held across a blocking read — `apply` takes the same one.
pub struct HackRfDevice {
    device: Arc<Mutex<HackRf>>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    running: Arc<AtomicBool>,
    /// Live only while streaming, and replaced by the capture thread on every restart, so
    /// `rx_stop` always reaches the stream that is actually running.
    stopper: Arc<Mutex<Option<Stopper>>>,
    worker: Option<JoinHandle<()>>,
}

impl HackRfDevice {
    fn new(device: HackRf) -> Self {
        let settings = caps::settings_from_config(device.config());
        Self {
            device: Arc::new(Mutex::new(device)),
            capabilities: caps::capabilities(),
            settings,
            running: Arc::new(AtomicBool::new(false)),
            stopper: Arc::new(Mutex::new(None)),
            worker: None,
        }
    }
}

fn write_to_hardware(device: &mut HackRf, applied: &caps::Applied) -> Result<(), DeviceError> {
    if let Some(hz) = applied.frequency_hz {
        device.set_frequency_hz(hz).map_err(map_err)?;
    }
    if let Some(rate) = applied.sample_rate_hz {
        device.set_sample_rate_hz(rate).map_err(map_err)?;
    }
    if let Some(db) = applied.lna_gain_db {
        device.set_lna_gain_db(db).map_err(map_err)?;
    }
    if let Some(db) = applied.vga_gain_db {
        device.set_vga_gain_db(db).map_err(map_err)?;
    }
    if let Some(enabled) = applied.amp {
        device.set_amp_enable(enabled).map_err(map_err)?;
    }
    if let Some(enabled) = applied.bias_tee {
        device.set_bias_tee(enabled).map_err(map_err)?;
    }
    Ok(())
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
        let (result, config) = {
            let mut device = lock(&self.device);
            let result = write_to_hardware(&mut device, &applied);
            (result, *device.config())
        };
        // The driver records a field only once its control transfer succeeded, so rebuilding
        // from `config()` reports exactly what the hardware holds: gains as the step grid
        // snapped them, and — when a batch failed halfway — the prefix that did land, never
        // the values that were asked for.
        self.settings = caps::settings_from_config(&config);
        result
    }

    fn rx_start(&mut self, sink: RxSink) -> Result<(), DeviceError> {
        if self.worker.is_some() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let stream = lock(&self.device).start_rx().map_err(map_err)?;
        *lock(&self.stopper) = Some(stream.stopper());
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        let device = self.device.clone();
        let stopper = self.stopper.clone();
        match std::thread::Builder::new()
            .name("sdrmm-hackrf-rx".to_string())
            .spawn(move || capture_loop(stream, &device, &stopper, &running, sink))
        {
            Ok(worker) => {
                self.worker = Some(worker);
                Ok(())
            }
            Err(e) => {
                // The un-spawned closure drops the stream, which releases the endpoint; turn
                // the radio back off and clear the state so a retry is not left looking like a
                // live capture.
                self.running.store(false, Ordering::Release);
                *lock(&self.stopper) = None;
                if let Err(stop) = lock(&self.device).stop_rx() {
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
        if let Err(e) = lock(&self.device).stop_rx() {
            tracing::debug!("hackrf stop_rx failed: {e}");
        }
        // Cloned rather than taken: the capture thread may be mid-restart and about to publish
        // a fresh stopper. It re-checks `running` after publishing, so whichever of the two
        // happens second still ends the stream and the join below cannot hang.
        let stopper = lock(&self.stopper).clone();
        if let Some(stopper) = stopper {
            stopper.stop();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        *lock(&self.stopper) = None;
    }
}

impl Drop for HackRfDevice {
    fn drop(&mut self) {
        self.rx_stop();
    }
}

/// Why a stream stopped delivering, when nobody asked it to.
struct Failure {
    reason: String,
    /// The radio left the bus. Restarting in place cannot help — the endpoint, the interface
    /// claim and the device handle went with it — so tier 2 is the only option.
    fatal: bool,
}

/// Blocking capture loop on the capture thread, and tier-1 supervisor for its stream.
///
/// Owns the stream, so every exit path drops it, releasing the device's bulk endpoint. A stream
/// that ends by itself is restarted in place under [`RestartPolicy`] — mode off, mode receive,
/// fresh queue — and only a restart that runs out of attempts reaches `sink.fail` and the
/// engine's destructive fault path. This backend needs it most: before the transport was
/// shared, a single errored transfer of any kind killed the stream outright.
fn capture_loop(
    mut stream: RxStream,
    device: &Mutex<HackRf>,
    stopper: &Mutex<Option<Stopper>>,
    running: &AtomicBool,
    mut sink: RxSink,
) {
    let mut converter = IqConverter::with_capacity(RX_TRANSFER_SIZE / 2);
    let mut policy = RestartPolicy::default();
    let mut dropped = 0u64;
    loop {
        let started = Instant::now();
        let Some(failure) = drain(&stream, running, &mut sink, &mut converter, &mut dropped) else {
            return;
        };
        if !running.load(Ordering::Acquire) {
            return;
        }
        if failure.fatal {
            sink.fail(DeviceError::Io(format!("device lost: {}", failure.reason)));
            return;
        }
        let Recovery::RetryAfter { attempt, delay } = policy.on_failure(started.elapsed()) else {
            sink.fail(DeviceError::Io(format!(
                "device lost after {} restart attempts: {}",
                policy.attempts() - 1,
                failure.reason
            )));
            return;
        };
        // A restart drops whatever the pipe had in flight, so it is never free and never silent.
        tracing::warn!(
            attempt,
            ?delay,
            reason = %failure.reason,
            "hackrf stream failed; restarting in place"
        );
        drop(stream);
        std::thread::sleep(delay);
        // A stalled transfer can complete on an odd length, and that half sample's partner is
        // never coming; carried into the fresh stream it would swap I and Q for good.
        converter.reset();
        let restarted = {
            let mut device = lock(device);
            // The radio is still in receive mode as far as the firmware knows, and `start_rx`
            // refuses a second stream; take it back to off first.
            device.stop_rx().and_then(|()| device.start_rx())
        };
        match restarted {
            Ok(fresh) => {
                // Published before the re-check, so a concurrent `rx_stop` either stops this
                // stream or is seen by the check below. One of the two always happens.
                *lock(stopper) = Some(fresh.stopper());
                if !running.load(Ordering::Acquire) {
                    return;
                }
                tracing::info!(attempt, "hackrf stream restarted");
                stream = fresh;
            }
            Err(e) => {
                sink.fail(DeviceError::Io(format!("stream restart failed: {e}")));
                return;
            }
        }
    }
}

/// Consume blocks until the stream ends or goes quiet. `None` means the caller asked to stop.
fn drain(
    stream: &RxStream,
    running: &AtomicBool,
    sink: &mut RxSink,
    converter: &mut IqConverter,
    dropped: &mut u64,
) -> Option<Failure> {
    let mut last_block = Instant::now();
    while running.load(Ordering::Acquire) {
        match stream.recv_timeout(RECV_POLL) {
            Ok(block) => {
                last_block = Instant::now();
                for chunk in converter.convert(&block).chunks(BLOCK_SAMPLES) {
                    sink.push(chunk);
                }
                // A dropped transfer is a gap in the sample stream that no counter downstream
                // can see, and is a different failure from the engine's ring overruns.
                let total = stream.stats().dropped;
                if total > *dropped {
                    tracing::warn!(dropped = total, "hackrf dropped usb transfers");
                    *dropped = total;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // A streaming HackRF free-runs and cannot go quiet while healthy, and an unplug
                // fails its queued transfers rather than going silent — so this fires only for
                // a wedged board, which would otherwise park this thread forever behind a dead
                // waterfall with no fault reported.
                if last_block.elapsed() >= SILENT_STREAM_TIMEOUT {
                    return Some(Failure {
                        reason: format!("no samples for {SILENT_STREAM_TIMEOUT:?}"),
                        fatal: false,
                    });
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Some(stream.error().map_or_else(
                    || Failure {
                        reason: "usb stream ended".to_string(),
                        fatal: false,
                    },
                    |error| Failure {
                        reason: error.to_string(),
                        fatal: error.is_disconnected(),
                    },
                ));
            }
        }
    }
    None
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
            map_err(driver::Error::DeviceNotFound),
            DeviceError::NotFound(_)
        ));
        // A value off the MAX2837 grid is "the hardware will not take this", not an I/O fault.
        assert!(matches!(
            map_err(driver::Error::InvalidConfig {
                field: "lna_gain_db",
                reason: "must be 0 through 40 dB in 8 dB steps",
            }),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(
            map_err(driver::Error::AlreadyStreaming),
            DeviceError::AlreadyStreaming
        ));
        assert!(matches!(
            map_err(driver::Error::ControlTransfer(
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
        assert_send::<HackRf>();
        assert_send::<RxStream>();
        assert_send::<HackRfDevice>();
    }

    // No test may call `HackRfDriver::probe`/`open`: both touch the USB bus, so their result
    // depends on what is plugged into the machine running them (PLAN §14: no hardware in CI,
    // ever). Everything above them is pure and tested here and in `caps`.
}
