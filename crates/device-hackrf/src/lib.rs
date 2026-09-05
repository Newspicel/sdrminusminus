use std::{
    sync::{Arc, Mutex, MutexGuard, mpsc::RecvTimeoutError},
    time::Duration,
};

use convert::samples_to_cs8;
use driver::{BurstQueue, DeviceDescriptor, FilterWidth, HackRf, SweepBlocks, TX_TRANSFER_SIZE};
pub use driver::{SweepPlan, SweepRange, SweepStyle};
use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, Direction, DuplexState,
    LutConverter, RxSink, Sample, SampleConverter, SdrDevice, SweepSink, TxStream, Worker, lock,
    single_rx_sink,
};
use sdrmm_usb_stream::{NusbBulkOut, RxStream};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod caps;
mod convert;
mod driver;

const DRIVER_ID: &str = "hackrf";
const SWEEP_READ_TIMEOUT: Duration = Duration::from_millis(100);
const SWEEP_MAX_RANGES: usize = 10;
const NOSERIAL_KEY_PREFIX: &str = "noserial-";

fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
    if err.is_disconnected() {
        return DeviceError::Disconnected(text);
    }
    match err {
        driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        driver::Error::InvalidConfig { .. } => DeviceError::Unsupported(text),
        _ => DeviceError::Io(text),
    }
}

fn full_serial(serial: u128) -> String {
    format!("{serial:032x}")
}

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

fn device_info(descriptor: &DeviceDescriptor) -> DeviceInfo {
    let serial = descriptor.serial.map(full_serial);
    // The bus/address pair rather than the enumeration index: it is stable while the radio stays
    // in the same port, where an index moves as soon as another device is plugged or unplugged
    // and would rebind a stored workspace to a different radio.
    let location = format!("{}/{}", descriptor.bus, descriptor.address);
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: serial
            .clone()
            .unwrap_or_else(|| format!("{NOSERIAL_KEY_PREFIX}{location}")),
        label: device_label(descriptor),
        serial,
        profile: Some(caps::capabilities().profile()),
    }
}

fn key_serial(key: &str) -> Option<u128> {
    u128::from_str_radix(key, 16).ok()
}

fn key_location(key: &str) -> Option<(String, u8)> {
    let (bus, address) = key.strip_prefix(NOSERIAL_KEY_PREFIX)?.rsplit_once('/')?;
    Some((bus.to_string(), address.parse().ok()?))
}

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
            Ok(found) => found.iter().map(device_info).collect(),
            Err(e) => {
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
    pub fn open_device(&self, info: &DeviceInfo) -> Result<HackRfDevice, DeviceError> {
        let device = match (key_serial(&info.key), key_location(&info.key)) {
            (Some(serial), _) => HackRf::open_serial(serial),
            // Opening "the first HackRF" would be the wrong radio as soon as two are attached,
            // and on this device that means driving the wrong transmitter.
            (None, Some((bus, address))) => HackRf::open_at(bus, address),
            (None, None) => {
                return Err(DeviceError::NotFound(format!(
                    "hackrf key {} names neither a serial nor a bus location",
                    info.key
                )));
            }
        }
        .map_err(map_err)?;
        Ok(HackRfDevice::new(device))
    }
}

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

    fn arm(&self) -> Result<RxStream, DeviceError> {
        let mut device = self.lock();
        device
            .set_mode_off()
            .and_then(|()| device.start_rx())
            .map_err(map_err)
    }

    fn disarm(&self) {
        if let Err(e) = self.lock().set_mode_off() {
            tracing::debug!("hackrf stop failed: {e}");
        }
    }
}

pub struct HackRfDevice {
    radio: Arc<HackRfRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    duplex: Arc<Mutex<DuplexState>>,
    capture: Capture<HackRfRadio>,
    sweeper: Worker,
}

impl HackRfDevice {
    fn new(device: HackRf) -> Self {
        let settings = caps::settings_from_config(device.config());
        let capabilities = caps::capabilities();
        Self {
            radio: Arc::new(HackRfRadio {
                device: Mutex::new(device),
            }),
            duplex: Arc::new(Mutex::new(DuplexState::new(capabilities.duplex))),
            capabilities,
            settings,
            capture: Capture::new(),
            sweeper: Worker::new(),
        }
    }
}

/// The firmware sweeps in whole megahertz, so a band is widened to the megahertz boundaries that
/// contain it rather than silently losing the edges the caller asked for. The firmware also holds
/// ten ranges at most, so the narrowest gaps are swallowed until the list fits — sweeping a little
/// spectrum nobody asked for beats refusing the plan.
fn firmware_ranges(plan: &sdrmm_device::SweepPlan) -> Result<Vec<SweepRange>, DeviceError> {
    plan.check()?;
    let mut ranges: Vec<SweepRange> = Vec::with_capacity(plan.bands.len());
    for band in &plan.bands {
        let start_mhz = (band.start_hz / 1e6).floor().max(1.0);
        let stop_mhz = (band.stop_hz / 1e6).ceil().max(start_mhz + 1.0);
        ranges.push(SweepRange {
            start_hz: (start_mhz as u64) * 1_000_000,
            stop_hz: (stop_mhz as u64) * 1_000_000,
        });
    }
    ranges.sort_by_key(|range| range.start_hz);
    while ranges.len() > SWEEP_MAX_RANGES {
        let seam = (1..ranges.len())
            .min_by_key(|&i| ranges[i].start_hz.saturating_sub(ranges[i - 1].stop_hz))
            .unwrap_or(1);
        let absorbed = ranges.remove(seam);
        ranges[seam - 1].stop_hz = ranges[seam - 1].stop_hz.max(absorbed.stop_hz);
    }
    Ok(ranges)
}

fn write_to_hardware(device: &mut HackRf, applied: &caps::Applied) -> Result<(), DeviceError> {
    if let Some(hz) = applied.frequency_hz {
        device.set_frequency_hz(hz).map_err(map_err)?;
    }
    if let Some(rate) = applied.sample_rate_hz {
        device.set_sample_rate_hz(rate).map_err(map_err)?;
    }
    match applied.filter {
        Some(FilterWidth::Hz(hz)) => device.set_filter_width_hz(hz).map_err(map_err)?,
        Some(FilterWidth::MatchRate) => device.set_filter_to_match_rate().map_err(map_err)?,
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
        self.settings = caps::settings_from_config(&config);
        result
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let sink = single_rx_sink(sinks)?;
        lock(&self.duplex).claim(Direction::Rx)?;
        let started = self.capture.start(
            self.radio.clone(),
            convert::converter(),
            sink,
            CaptureConfig::new("sdrmm-hackrf-rx", DRIVER_ID)
                .with_sample_rate(self.settings.sample_rate),
        );
        if started.is_err() {
            lock(&self.duplex).release(Direction::Rx);
        }
        started
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
        lock(&self.duplex).release(Direction::Rx);
    }

    fn sweep_start(
        &mut self,
        plan: &sdrmm_device::SweepPlan,
        mut sink: SweepSink,
    ) -> Result<(), DeviceError> {
        let ranges = firmware_ranges(plan)?;
        let rate = u32::try_from(plan.sample_rate_hz.round() as i64).map_err(|_| {
            DeviceError::Unsupported(format!("sweep sample rate {} Hz", plan.sample_rate_hz))
        })?;
        self.radio
            .lock()
            .set_sample_rate_hz(rate)
            .map_err(map_err)?;
        let firmware = SweepPlan::interleaved(ranges, rate & !3);
        let mut sweep = self.sweep_start(&firmware)?;
        self.sweeper
            .start("sdrmm-hackrf-sweep", move |running| {
                while running.load(std::sync::atomic::Ordering::Acquire) {
                    match sweep.read(SWEEP_READ_TIMEOUT, |capture| {
                        sink.push(capture.tuned_hz as f64, capture.samples);
                    }) {
                        Ok(_) => {}
                        Err(e) => {
                            sink.fail(e);
                            return;
                        }
                    }
                }
            })
            .inspect_err(|_| self.sweep_stop())
    }

    fn sweep_stop(&mut self) {
        self.sweeper.stop();
    }

    fn tx_start_channels(&mut self, channels: &[u32]) -> Result<Box<dyn TxStream>, DeviceError> {
        if channels != [0] {
            return Err(DeviceError::Unsupported(format!(
                "this device has 1 tx stream, got channels {channels:?}"
            )));
        }
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

    pub fn set_tx_gain_db(&mut self, gain_db: u8) -> Result<(), DeviceError> {
        self.radio
            .lock()
            .set_tx_vga_gain_db(gain_db)
            .map_err(map_err)
    }
}

#[derive(Debug)]
pub struct SweepCapture<'a> {
    pub stamp_hz: u64,
    pub tuned_hz: u64,
    pub samples: &'a [Sample],
}

struct SweepDecoder {
    converter: LutConverter,
    offset_hz: u64,
    skipped: u64,
}

impl SweepDecoder {
    fn new(offset_hz: u64) -> Self {
        Self {
            converter: convert::sweep_converter(),
            offset_hz,
            skipped: 0,
        }
    }

    fn decode(&mut self, transfer: &[u8], mut visit: impl FnMut(SweepCapture<'_>)) -> usize {
        let mut delivered = 0;
        let mut blocks = SweepBlocks::new(transfer, self.offset_hz);
        for block in blocks.by_ref() {
            visit(SweepCapture {
                stamp_hz: block.stamp_hz,
                tuned_hz: block.tuned_hz,
                samples: self.converter.convert(block.iq),
            });
            delivered += 1;
        }
        let skipped = blocks.skipped();

        if skipped > 0 {
            self.skipped += skipped as u64;
            tracing::warn!(
                skipped,
                total = self.skipped,
                "hackrf sweep transfer carried blocks that were not sweep frames"
            );
        }
        delivered
    }

    const fn skipped_total(&self) -> u64 {
        self.skipped
    }
}

pub struct HackRfSweep {
    radio: Arc<HackRfRadio>,
    duplex: Arc<Mutex<DuplexState>>,
    stream: Option<RxStream>,
    decoder: SweepDecoder,
}

impl HackRfSweep {
    /// Blocks this sweep has seen that were not sweep frames. Non-zero means the firmware or the
    /// transfer framing lost data, which a caller has to surface rather than read as a gap.
    #[must_use]
    pub const fn skipped(&self) -> u64 {
        self.decoder.skipped_total()
    }

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

    pub fn stop(&mut self) -> Result<(), DeviceError> {
        let Some(mut stream) = self.stream.take() else {
            return Ok(());
        };
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

struct HackRfTx {
    radio: Arc<HackRfRadio>,
    duplex: Arc<Mutex<DuplexState>>,
    queue: Option<BurstQueue<NusbBulkOut>>,
    bytes: Vec<u8>,
}

impl HackRfTx {
    fn release(&mut self, queue: BurstQueue<NusbBulkOut>) -> Result<(), DeviceError> {
        tracing::info!(stats = ?queue.stats(), "hackrf transmit finished");
        drop(queue);
        let stopped = self.radio.lock().set_mode_off();
        lock(&self.duplex).release(Direction::Tx);
        stopped.map_err(map_err)
    }
}

impl TxStream for HackRfTx {
    fn write_channels(
        &mut self,
        channels: &[&[Sample]],
        timeout: Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError> {
        let [samples] = channels else {
            return Err(DeviceError::Unsupported(format!(
                "this device has 1 tx stream, got {} channels",
                channels.len()
            )));
        };
        let Some(queue) = self.queue.as_mut() else {
            return Err(DeviceError::Io("transmit stream is stopped".to_string()));
        };
        samples_to_cs8(samples, &mut self.bytes);
        let bytes = std::mem::take(&mut self.bytes);
        let accepted = queue.write(&bytes, timeout, end_burst);
        self.bytes = bytes;
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
        let Some(mut queue) = self.queue.take() else {
            return;
        };
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
            bus: "20".to_string(),
            address: 7,
        }
    }

    #[test]
    fn key_is_the_full_usb_serial_so_the_registry_can_merge() {
        let info = device_info(&descriptor(Some(SERIAL), None));
        assert_eq!(info.driver, "hackrf");
        assert_eq!(info.key, "0000000000000000675c62dc3b2d4b8b");
        assert_eq!(
            info.serial.as_deref(),
            Some("0000000000000000675c62dc3b2d4b8b")
        );
        assert_eq!(info.id(), "hackrf:0000000000000000675c62dc3b2d4b8b");
        assert_eq!(info.key.len(), 32);
    }

    #[test]
    fn label_shows_the_short_serial_users_recognise() {
        assert_eq!(
            device_label(&descriptor(Some(SERIAL), None)),
            "HackRF One / HackRF Pro 675c62dc3b2d4b8b"
        );
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
    fn a_serialless_device_is_keyed_by_where_it_is_plugged_in() {
        let info = device_info(&descriptor(None, Some("rad1o")));
        assert_eq!(info.key, "noserial-20/7");
        assert_eq!(info.serial, None);
        assert_eq!(info.label, "rad1o");
        assert_eq!(key_serial(&info.key), None);
    }

    #[test]
    fn a_serialless_key_round_trips_to_the_bus_location_open_needs() {
        let info = device_info(&descriptor(None, None));
        assert_eq!(key_serial(&info.key), None);
        assert_eq!(key_location(&info.key), Some(("20".to_string(), 7)));
    }

    #[test]
    fn a_key_naming_neither_a_serial_nor_a_location_is_refused() {
        assert_eq!(key_location("noserial-nonsense"), None);
        assert_eq!(key_location("noserial-20/notanumber"), None);
        assert_eq!(key_location("20/7"), None);
    }

    #[test]
    fn keys_round_trip_back_to_the_serial_open_needs() {
        let info = device_info(&descriptor(Some(SERIAL), None));
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
            DeviceError::Disconnected(_),
        ));
        assert!(matches!(
            map_err(driver::Error::ControlTransfer(
                nusb::transfer::TransferError::Stall
            )),
            DeviceError::Io(_)
        ));
    }

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
        state.release(Direction::Rx);
        state.claim(Direction::Tx).expect("transmit");
    }

    #[test]
    fn a_band_widens_to_the_whole_megahertz_the_firmware_steps_in() {
        let plan = sdrmm_device::SweepPlan::new(
            vec![sdrmm_device::SweepBand {
                start_hz: 88_500_000.0,
                stop_hz: 107_900_000.0,
            }],
            20_000_000.0,
        );
        assert_eq!(
            firmware_ranges(&plan).expect("a coverable band"),
            vec![SweepRange {
                start_hz: 88_000_000,
                stop_hz: 108_000_000,
            }],
            "narrowing a band would leave the edges unswept"
        );
    }

    #[test]
    fn a_band_thinner_than_a_step_still_gets_a_megahertz_to_sweep() {
        let plan = sdrmm_device::SweepPlan::new(
            vec![sdrmm_device::SweepBand {
                start_hz: 433_050_000.0,
                stop_hz: 433_100_000.0,
            }],
            20_000_000.0,
        );
        let ranges = firmware_ranges(&plan).expect("a narrow band");
        assert_eq!(ranges[0].start_hz, 433_000_000);
        assert!(
            ranges[0].stop_hz > ranges[0].start_hz,
            "an empty range stalls the firmware"
        );
    }

    #[test]
    fn more_bands_than_the_firmware_holds_are_merged_at_the_narrowest_gaps() {
        let bands = (0..14)
            .map(|i| sdrmm_device::SweepBand {
                start_hz: 100e6 + f64::from(i) * 10e6,
                stop_hz: 101e6 + f64::from(i) * 10e6,
            })
            .chain(std::iter::once(sdrmm_device::SweepBand {
                start_hz: 2_400e6,
                stop_hz: 2_401e6,
            }))
            .collect();
        let ranges =
            firmware_ranges(&sdrmm_device::SweepPlan::new(bands, 20_000_000.0)).expect("merged");
        assert_eq!(ranges.len(), SWEEP_MAX_RANGES, "the firmware holds ten");
        assert_eq!(
            ranges[0].start_hz, 100_000_000,
            "the bottom of the plan must still be swept"
        );
        assert_eq!(
            ranges[SWEEP_MAX_RANGES - 1].stop_hz,
            2_401_000_000,
            "the far band must not be the one dropped"
        );
        assert!(
            ranges.windows(2).all(|w| w[0].stop_hz <= w[1].start_hz),
            "merging must not leave overlapping ranges: {ranges:?}"
        );
    }

    #[test]
    fn a_plan_the_generic_check_refuses_never_reaches_the_firmware() {
        let plan = sdrmm_device::SweepPlan::new(Vec::new(), 20_000_000.0);
        assert!(firmware_ranges(&plan).is_err());
    }

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

    #[test]
    fn an_unframed_transfer_decodes_to_nothing() {
        let mut decoder = SweepDecoder::new(0);
        let delivered = decoder.decode(&vec![0u8; 16_384], |_| panic!("nothing to decode"));
        assert_eq!(delivered, 0);
    }

    #[test]
    fn a_transmit_burst_is_a_boxed_tx_stream() {
        const fn assert_tx_stream<T: TxStream + 'static>() {}
        assert_tx_stream::<HackRfTx>();
    }

    #[test]
    fn device_and_stream_cross_thread_boundaries() {
        const fn assert_send<T: Send>() {}
        assert_send::<HackRf>();
        assert_send::<RxStream>();
        assert_send::<HackRfDevice>();
    }
}
