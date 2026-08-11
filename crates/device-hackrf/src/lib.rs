//! `sdrmm-device-hackrf` — native HackRF backend (PLAN §6, feature `hackrf-native`): the HackRF
//! driver and the `SdrDevice` implementation over it, pure Rust on `nusb`, so release artifacts
//! launch with no libhackrf, no libSoapySDR and no C dependency at all (PLAN §15).
//!
//! What it buys over the Soapy view of the same radio is the real per-stage gain model — LNA
//! and VGA separately, each on its own MAX2837 step grid — the baseband filter as a width of its
//! own rather than a shadow of the sample rate, and the RF amplifier and antenna-port bias power
//! as typed extras. The firmware's self-retuning sweep is here too, as
//! [`HackRfDevice::sweep_start`]; nothing above this crate drives it yet.
//!
//! Four layers, in dependency order:
//!
//! - [`driver`] — the radio: enumeration, the vendor control protocol, both stream lifecycles.
//!   No wire types, and no arbitration.
//! - `convert` — the table that turns the HackRF's signed 8-bit codes into `cf32`, and back.
//! - `caps` — the pure translation to the wire capability model, and `apply`'s validation.
//! - this module — `DeviceDriver`/`SdrDevice` over `sdrmm-device`'s shared machinery.
//!
//! Streaming lives in `sdrmm-usb-stream` and supervision in `sdrmm-device`, both shared with the
//! RTL-SDR backend: the transfer queues, the USB error policy, the restart loop and the
//! half-duplex rule are what radios have in common, and getting them wrong on each side
//! separately is the defect this driver exists to fix (PLAN §17).
//!
//! The radio is half duplex, which here means exactly one thing: a
//! [`DuplexState`](sdrmm_device::DuplexState) decides whether a direction may start, and each
//! direction releases only its own claim when it ends. Stopping a capture cannot silence a
//! transmit burst, and vice versa.

use std::{
    sync::{Arc, Mutex, MutexGuard, mpsc::RecvTimeoutError},
    time::Duration,
};

use convert::samples_to_cs8;
use driver::{BurstQueue, DeviceDescriptor, HackRf, SweepBlocks, TX_TRANSFER_SIZE};
pub use driver::{SweepPlan, SweepRange, SweepStyle};
use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, Direction, DuplexState,
    LutConverter, RxSink, Sample, SampleConverter, SdrDevice, TxStream, lock, single_rx_sink,
};
use sdrmm_usb_stream::{NusbBulkOut, RxStream};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod caps;
mod convert;
mod driver;

const DRIVER_ID: &str = "hackrf";
/// Key prefix for a HackRF whose USB descriptor carries no parseable serial. Prefixed so it
/// can never collide with a hex serial, and index-based because there is nothing else left
/// to key on — see [`HackRfDriver::open`].
const NOSERIAL_KEY_PREFIX: &str = "noserial-";

/// The driver's error taxonomy onto the four `DeviceError`s. `InvalidConfig` becomes
/// `Unsupported` because it means "the hardware will not take this value", which is what the
/// control plane renders. Everything USB-level is I/O: the engine's fault path treats any
/// capture error as a lost device and re-opens it when it enumerates again.
fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
    match err {
        driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        driver::Error::InvalidConfig { .. } => DeviceError::Unsupported(text),
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
        // A HackRF is a HackRF: everything the picker filters on is the model's, not the unit's,
        // so a template can be matched against it without claiming the radio first.
        profile: Some(caps::capabilities().profile()),
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
        Ok(Box::new(self.open_device(info)?))
    }
}

impl HackRfDriver {
    /// Open a probed radio as its concrete type.
    ///
    /// [`DeviceDriver::open`] erases it to `dyn SdrDevice`, which carries `tx_start` but not the
    /// transmit *gain* — that has no wire setting while transmit is gated (PLAN §12a) — so this is how
    /// [`HackRfDevice::set_tx_gain_db`] is reached at all.
    ///
    /// # Errors
    /// [`DeviceError::NotFound`] if the radio is gone, [`DeviceError::Io`] if it will not open.
    pub fn open_device(&self, info: &DeviceInfo) -> Result<HackRfDevice, DeviceError> {
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
        Ok(HackRfDevice::new(device))
    }
}

/// The radio, as the shared capture supervisor sees it.
///
/// Behind a mutex because both threads need it: the control thread retunes through it while the
/// capture thread re-arms the stream from it. The lock is never held across a blocking read —
/// the transport's [`RxStream`] holds its own bulk endpoint and borrows nothing from here, so
/// `apply` retunes through the control endpoint while samples keep flowing.
struct HackRfRadio {
    device: Mutex<HackRf>,
}

impl HackRfRadio {
    fn lock(&self) -> MutexGuard<'_, HackRf> {
        lock(&self.device)
    }
}

impl CaptureRadio for HackRfRadio {
    type Stream = RxStream;

    /// The radio is still in receive mode as far as the firmware knows after a stream faults, so
    /// this takes it back to off before filling a fresh queue — which makes the same call serve
    /// a cold start and an in-place restart.
    fn arm(&self) -> Result<RxStream, DeviceError> {
        let mut device = self.lock();
        device
            .set_mode_off()
            .and_then(|()| device.start_rx())
            .map_err(map_err)
    }

    /// Only ever called for a capture this backend started, and only for the receive direction —
    /// [`Capture`] holds a radio exactly while one is running, so a device that is transmitting
    /// has nothing here to switch off.
    fn disarm(&self) {
        if let Err(e) = self.lock().set_mode_off() {
            tracing::debug!("hackrf stop failed: {e}");
        }
    }
}

/// An opened HackRF.
pub struct HackRfDevice {
    radio: Arc<HackRfRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// Which direction holds the radio. Shared with a live [`HackRfTx`], because a burst outlives
    /// the call that started it and has to give its claim back when it ends.
    duplex: Arc<Mutex<DuplexState>>,
    capture: Capture<HackRfRadio>,
}

impl HackRfDevice {
    fn new(device: HackRf) -> Self {
        let settings = caps::settings_from_config(device.config());
        let capabilities = caps::capabilities();
        Self {
            radio: Arc::new(HackRfRadio {
                device: Mutex::new(device),
            }),
            // One transceiver, one data path: the LPC's mode register selects a direction, so the
            // other one has to stop first. Declared once, in the capabilities the client renders
            // from, so the arbitration cannot promise something the ports do not.
            duplex: Arc::new(Mutex::new(DuplexState::new(capabilities.duplex))),
            capabilities,
            settings,
            capture: Capture::new(),
        }
    }
}

fn write_to_hardware(device: &mut HackRf, applied: &caps::Applied) -> Result<(), DeviceError> {
    if let Some(hz) = applied.frequency_hz {
        device.set_frequency_hz(hz).map_err(map_err)?;
    }
    // Rate before filter, always: setting the rate moves the filter to match, so a width
    // written first would be overwritten by the rate that came after it in the same delta.
    if let Some(rate) = applied.sample_rate_hz {
        device.set_sample_rate_hz(rate).map_err(map_err)?;
    }
    match applied.filter {
        Some(caps::FilterWidth::Hz(hz)) => device.set_filter_width_hz(hz).map_err(map_err)?,
        Some(caps::FilterWidth::MatchRate) => device.set_filter_to_match_rate().map_err(map_err)?,
        None => {}
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
            let mut device = self.radio.lock();
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

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        // Before the duplex claim, so a refused sink count cannot leak a receive claim.
        let sink = single_rx_sink(sinks)?;
        lock(&self.duplex).claim(Direction::Rx)?;
        let started = self.capture.start(
            self.radio.clone(),
            convert::converter(),
            sink,
            CaptureConfig::new("sdrmm-hackrf-rx", DRIVER_ID),
        );
        if started.is_err() {
            lock(&self.duplex).release(Direction::Rx);
        }
        started
    }

    /// Stops a capture, and *only* a capture. A `Capture` that is not running holds no radio, so
    /// this reaches neither the transceiver mode nor a transmit claim — which is what keeps a
    /// stray `rx_stop` from silencing a burst that is on the air.
    fn rx_stop(&mut self) {
        self.capture.stop();
        lock(&self.duplex).release(Direction::Rx);
    }

    /// Claim the radio for transmit.
    ///
    /// Nothing above this crate calls it: the transmit input is inert (CANVAS, `PortType::Tx`), and PLAN
    /// §12a puts every application-level transmit feature behind an explicit authorized-use
    /// switch that has not been built. Radiating is the operator's responsibility — a HackRF
    /// transmits wideband into whatever is on the antenna port, and most of its range is
    /// licensed to somebody.
    ///
    /// # Errors
    /// [`DeviceError::DuplexConflict`] while a capture is running — the radio is half duplex, so
    /// `rx_stop` has to come first — or [`DeviceError::Io`] if the radio will not start.
    fn tx_start(&mut self) -> Result<Box<dyn TxStream>, DeviceError> {
        lock(&self.duplex).claim(Direction::Tx)?;
        match self.radio.lock().start_tx() {
            Ok(queue) => Ok(Box::new(HackRfTx {
                radio: self.radio.clone(),
                duplex: self.duplex.clone(),
                queue: Some(queue),
                bytes: Vec::with_capacity(TX_TRANSFER_SIZE),
            })),
            Err(e) => {
                lock(&self.duplex).release(Direction::Tx);
                Err(map_err(e))
            }
        }
    }
}

impl HackRfDevice {
    /// Hand the tuning to the firmware and start sweeping.
    ///
    /// A sweep is a receive stream that retunes itself, so it takes the receive claim and is
    /// arbitrated against everything else the radio could be doing: a capture running here is
    /// [`DeviceError::AlreadyStreaming`], and a burst on the air is a
    /// [`DeviceError::DuplexConflict`]. It deliberately does *not* go through [`Capture`]: that
    /// supervisor's restart path re-arms with `start_rx`, which would silently drop a faulted
    /// sweep back into a plain capture on one frequency — so a sweep that faults surfaces the
    /// fault instead, and the caller decides.
    ///
    /// Not a wire setting, and reachable only from Rust — like [`SdrDevice::tx_start`]. Driving
    /// it from the scanner is its own piece of work (FEATURES §4, "hardware-assisted wideband
    /// sweep"), because the scanner measures off the device set's spectrum tap and a sweep
    /// produces a different shape of data entirely.
    ///
    /// The sample rate must not change while the sweep runs: a plan's step and offset are
    /// geometry derived from the rate the caller built it at, and the firmware would keep
    /// walking the old grid over a new passband. Nothing enforces it here, because nothing
    /// above this crate can hold a sweep and an [`SdrDevice::apply`] at the same time — whoever
    /// wires §4 owns the device and owns this rule with it.
    ///
    /// # Errors
    /// [`DeviceError::Unsupported`] for a plan the firmware would refuse or firmware too old to
    /// sweep, and [`DeviceError::Io`] if the radio will not start.
    pub fn sweep_start(&mut self, plan: &SweepPlan) -> Result<HackRfSweep, DeviceError> {
        lock(&self.duplex).claim(Direction::Rx)?;
        match self.radio.lock().start_rx_sweep(plan) {
            Ok(stream) => Ok(HackRfSweep {
                radio: self.radio.clone(),
                duplex: self.duplex.clone(),
                stream: Some(stream),
                decoder: SweepDecoder::new(u64::from(plan.offset_hz)),
            }),
            Err(e) => {
                lock(&self.duplex).release(Direction::Rx);
                Err(map_err(e))
            }
        }
    }

    /// The transmit VGA, 0–47 dB. It powers up at zero and is set back to zero when the device
    /// is opened, so the radio cannot be made to radiate at drive by a mode change alone.
    ///
    /// Not a wire setting: `Capabilities` advertises no transmit gain stage while transmit
    /// is false, so this is reachable only from Rust, like [`SdrDevice::tx_start`] itself.
    ///
    /// # Errors
    /// [`DeviceError::Unsupported`] above 47 dB; [`DeviceError::Io`] if the radio refuses it.
    pub fn set_tx_gain_db(&mut self, gain_db: u8) -> Result<(), DeviceError> {
        self.radio
            .lock()
            .set_tx_vga_gain_db(gain_db)
            .map_err(map_err)
    }
}

/// One located capture from a running sweep.
#[derive(Debug)]
pub struct SweepCapture<'a> {
    /// The firmware's own stamp: the low edge of the span this capture covers.
    pub stamp_hz: u64,
    /// Where the radio was — the stamp plus the plan's tuning offset.
    pub tuned_hz: u64,
    /// The capture. `hackrf_sweep` takes its FFT window from the *end* of this, because the
    /// retune that precedes a block is still settling at the start of it.
    pub samples: &'a [Sample],
}

/// The sample side of a sweep: one USB transfer in, its located captures out.
///
/// Split from [`HackRfSweep`] because it is the half with no radio in it, and therefore the half
/// that can be tested at all (PLAN §14: no hardware in CI, ever). Everything the transport half
/// adds is a `recv` and an error mapping.
struct SweepDecoder {
    /// Reused across blocks, so a running sweep allocates nothing.
    converter: LutConverter,
    offset_hz: u64,
}

impl SweepDecoder {
    fn new(offset_hz: u64) -> Self {
        Self {
            converter: convert::sweep_converter(),
            offset_hz,
        }
    }

    fn decode(&mut self, transfer: &[u8], mut visit: impl FnMut(SweepCapture<'_>)) -> usize {
        let mut delivered = 0;
        for block in SweepBlocks::new(transfer, self.offset_hz) {
            visit(SweepCapture {
                stamp_hz: block.stamp_hz,
                tuned_hz: block.tuned_hz,
                samples: self.converter.convert(block.iq),
            });
            delivered += 1;
        }
        delivered
    }
}

/// A running sweep. Dropping it takes the radio out of sweep mode.
pub struct HackRfSweep {
    radio: Arc<HackRfRadio>,
    /// Released when the sweep ends, and only the receive half of it.
    duplex: Arc<Mutex<DuplexState>>,
    /// Taken by [`HackRfSweep::stop`]; `None` afterwards, so a stopped sweep neither yields more
    /// blocks nor tears the radio down twice.
    stream: Option<RxStream>,
    decoder: SweepDecoder,
}

impl HackRfSweep {
    /// Wait up to `timeout` for the next transfer and hand each located capture in it to
    /// `visit`, returning how many there were.
    ///
    /// Zero means the timeout expired with nothing to show, which is not an error: it is how a
    /// caller stays responsive to its own stop flag while the firmware works through a range
    /// whose blocks the transport has not filled a transfer with yet.
    ///
    /// A visitor rather than an iterator because the sample buffer is reused between captures:
    /// two blocks from one transfer are two different frequencies, and handing both out at once
    /// would mean either an allocation per block or a caller holding a slice that has already
    /// been overwritten.
    ///
    /// # Errors
    /// [`DeviceError::Io`] once the stream is over — a fault, or a [`HackRfSweep::stop`] that
    /// already happened.
    pub fn read(
        &mut self,
        timeout: Duration,
        visit: impl FnMut(SweepCapture<'_>),
    ) -> Result<usize, DeviceError> {
        let Some(stream) = self.stream.as_ref() else {
            return Err(DeviceError::Io("sweep is stopped".to_string()));
        };
        let transfer = match stream.recv_timeout(timeout) {
            Ok(transfer) => transfer,
            Err(RecvTimeoutError::Timeout) => return Ok(0),
            Err(RecvTimeoutError::Disconnected) => {
                let reason = stream.error().map_or_else(
                    || "sweep stream ended".to_string(),
                    |error| format!("sweep stream failed: {error}"),
                );
                return Err(DeviceError::Io(reason));
            }
        };
        Ok(self.decoder.decode(&transfer, visit))
    }

    /// Stop sweeping and give the radio back. Idempotent.
    ///
    /// # Errors
    /// [`DeviceError::Io`] if the radio would not leave sweep mode. The claim is released
    /// either way — a radio nobody can talk to must not also be a radio nobody can re-open.
    pub fn stop(&mut self) -> Result<(), DeviceError> {
        let Some(mut stream) = self.stream.take() else {
            return Ok(());
        };
        // Mode off first: the firmware would otherwise keep filling transfers the pump is no
        // longer draining, which is exactly the backlog `set_mode_off` before teardown avoids on
        // the plain receive path.
        let stopped = self.radio.lock().set_mode_off();
        tracing::info!(stats = ?stream.stop(), "hackrf sweep finished");
        lock(&self.duplex).release(Direction::Rx);
        stopped.map_err(map_err)
    }
}

impl Drop for HackRfSweep {
    fn drop(&mut self) {
        if let Err(e) = self.stop() {
            tracing::debug!("hackrf sweep stop failed: {e}");
        }
    }
}

/// A live transmit burst. Dropping it silences the radio.
struct HackRfTx {
    radio: Arc<HackRfRadio>,
    /// Released when the burst ends, and only the transmit half of it: a receive stream that
    /// started afterwards is none of this type's business.
    duplex: Arc<Mutex<DuplexState>>,
    /// Taken by [`TxStream::stop`]; `None` afterwards, so a stopped burst neither accepts more
    /// samples nor tears the radio down a second time.
    queue: Option<BurstQueue<NusbBulkOut>>,
    /// Reused across writes, so a steady burst allocates nothing.
    bytes: Vec<u8>,
}

impl HackRfTx {
    /// Give the radio and the claim back. `queue` must already be taken.
    fn release(&mut self, queue: BurstQueue<NusbBulkOut>) -> Result<(), DeviceError> {
        tracing::info!(stats = ?queue.stats(), "hackrf transmit finished");
        drop(queue);
        let stopped = self.radio.lock().set_mode_off();
        lock(&self.duplex).release(Direction::Tx);
        stopped.map_err(map_err)
    }
}

impl TxStream for HackRfTx {
    fn write(
        &mut self,
        samples: &[Sample],
        timeout: Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError> {
        let Some(queue) = self.queue.as_mut() else {
            return Err(DeviceError::Io("transmit stream is stopped".to_string()));
        };
        samples_to_cs8(samples, &mut self.bytes);
        let bytes = std::mem::take(&mut self.bytes);
        let accepted = queue.write(&bytes, timeout, end_burst);
        self.bytes = bytes;
        // Every chunk boundary is an even byte count, so a partial accept is still a whole
        // number of samples.
        accepted.map(|bytes| bytes / 2).map_err(map_err)
    }

    fn stop(&mut self) -> Result<(), DeviceError> {
        let Some(mut queue) = self.queue.take() else {
            return Ok(());
        };
        let drained = queue.flush_and_drain();
        if drained.is_err() {
            queue.abort();
        }
        let stopped = self.release(queue);
        drained.map_err(map_err)?;
        stopped
    }
}

impl Drop for HackRfTx {
    fn drop(&mut self) {
        // `stop` already gave the radio back, and something else may hold it by now — touching
        // the transceiver mode here would silence whatever that is.
        let Some(mut queue) = self.queue.take() else {
            return;
        };
        // Never wait: a dropped burst is an abandoned one, and the only thing that matters is
        // that the antenna goes quiet.
        queue.abort();
        if let Err(e) = self.release(queue) {
            tracing::debug!("hackrf tx stop failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::Duplex;

    use super::*;

    const SERIAL: u128 = 0x0000_0000_0000_0000_675c_62dc_3b2d_4b8b;

    /// One firmware sweep block: the magic, the stamp, then `code` for every IQ byte.
    fn sweep_block(stamp_hz: u64, code: u8) -> Vec<u8> {
        let mut bytes = vec![0x7f, 0x7f];
        bytes.extend_from_slice(&stamp_hz.to_le_bytes());
        bytes.resize(16_384, code);
        bytes
    }

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
            map_err(driver::Error::ControlTransfer(
                nusb::transfer::TransferError::Disconnected
            )),
            DeviceError::Io(_)
        ));
    }

    /// This crate's whole contribution to the half-duplex rule is the declaration; the rule
    /// itself is `sdrmm-device`'s and tested there. Pinning it here is what stops a later edit
    /// from quietly promising a radio it can do both at once.
    #[test]
    fn the_radio_declares_itself_half_duplex() {
        let declared = caps::capabilities().duplex;
        let mut state = DuplexState::new(declared);
        assert_eq!(declared, Duplex::Half);
        assert!(declared.supports(Direction::Rx));
        assert!(declared.supports(Direction::Tx));
        assert!(!declared.simultaneous());
        state.claim(Direction::Rx).expect("receive");
        assert!(matches!(
            state.claim(Direction::Tx),
            Err(DeviceError::DuplexConflict { .. })
        ));
        // Releasing the receive claim frees the path, and nothing else.
        state.release(Direction::Rx);
        state.claim(Direction::Tx).expect("transmit");
    }

    /// Sixteen blocks per transfer, each stamped and converted on its own — a sweep's samples
    /// are only meaningful next to the frequency they were captured at, so the reader must
    /// never hand out a run that spans two of them.
    #[test]
    fn a_sweep_transfer_decodes_into_one_capture_per_block() {
        let mut transfer = sweep_block(88_000_000, 0x7f);
        transfer.extend(sweep_block(93_000_000, 0x80));
        let mut decoder = SweepDecoder::new(7_500_000);
        let mut seen = Vec::new();
        let delivered = decoder.decode(&transfer, |capture| {
            seen.push((
                capture.stamp_hz,
                capture.tuned_hz,
                capture.samples.len(),
                capture.samples[0],
            ));
        });
        assert_eq!(delivered, 2);
        assert_eq!(
            seen,
            vec![
                // 0x7f and 0x80 are the extremes of the signed coding, so this also pins that
                // the sweep path reads the same table the capture path does.
                (
                    88_000_000,
                    95_500_000,
                    8_187,
                    Sample::new(127.0 / 128.0, 127.0 / 128.0)
                ),
                (93_000_000, 100_500_000, 8_187, Sample::new(-1.0, -1.0)),
            ]
        );
    }

    /// The decoder is fed whole transfers for the life of a sweep; a per-block allocation would
    /// be one every 8187 samples at 20 Msps.
    #[test]
    fn a_running_sweep_does_not_allocate_per_block() {
        let transfer = sweep_block(88_000_000, 0x00);
        let mut decoder = SweepDecoder::new(0);
        let mut first = None;
        decoder.decode(&transfer, |capture| first = Some(capture.samples.as_ptr()));
        let mut second = None;
        decoder.decode(&transfer, |capture| second = Some(capture.samples.as_ptr()));
        assert_eq!(first, second);
    }

    /// A transfer the firmware never framed yields nothing rather than samples at a guessed
    /// frequency — the reader's count is what tells a caller its sweep is not producing.
    #[test]
    fn an_unframed_transfer_decodes_to_nothing() {
        let mut decoder = SweepDecoder::new(0);
        let delivered = decoder.decode(&vec![0u8; 16_384], |_| panic!("nothing to decode"));
        assert_eq!(delivered, 0);
    }

    /// `tx_start` hands back a trait object, so the burst has to be object-safe — the property
    /// that lets the abstract layer carry transmit at all.
    #[test]
    fn a_transmit_burst_is_a_boxed_tx_stream() {
        const fn assert_tx_stream<T: TxStream + 'static>() {}
        assert_tx_stream::<HackRfTx>();
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
