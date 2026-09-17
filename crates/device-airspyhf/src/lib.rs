use std::sync::{Arc, Mutex, MutexGuard};

use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, Direction, DuplexState,
    RxSink, SdrDevice, lock, single_rx_sink,
};
use sdrmm_usb_stream::RxStream;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, GainKind};

use crate::driver::{AirspyHf, DeviceDescriptor, RX_TRANSFER_SIZE};

mod caps;
mod convert;
mod driver;

const DRIVER_ID: &str = "airspyhf";
const NOSERIAL_KEY_PREFIX: &str = "noserial-";

fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
    if err.is_disconnected() {
        return DeviceError::Disconnected(text);
    }
    if err.is_permission_denied() {
        return DeviceError::PermissionDenied(text);
    }
    match err {
        driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        driver::Error::InvalidConfig { .. } => DeviceError::Unsupported(text),
        _ => DeviceError::Io(text),
    }
}

fn full_serial(serial: u64) -> String {
    format!("{serial:016x}")
}

fn device_label(descriptor: &DeviceDescriptor) -> String {
    let name = descriptor
        .product_string
        .as_deref()
        .unwrap_or(descriptor.description);
    match descriptor.serial {
        Some(serial) => format!("{name} {}", full_serial(serial)),
        None => name.to_string(),
    }
}

fn device_info(descriptor: &DeviceDescriptor) -> DeviceInfo {
    let serial = descriptor.serial.map(full_serial);
    let location = format!("{}/{}", descriptor.bus, descriptor.address);
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: serial
            .clone()
            .unwrap_or_else(|| format!("{NOSERIAL_KEY_PREFIX}{location}")),
        label: device_label(descriptor),
        serial,
        profile: None,
    }
}

fn key_serial(key: &str) -> Option<u64> {
    if key.len() != 16 {
        return None;
    }
    u64::from_str_radix(key, 16).ok()
}

fn key_location(key: &str) -> Option<(String, u8)> {
    let (bus, address) = key.strip_prefix(NOSERIAL_KEY_PREFIX)?.rsplit_once('/')?;
    Some((bus.to_string(), address.parse().ok()?))
}

#[derive(Default)]
pub struct AirspyHfDriver;

impl AirspyHfDriver {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn open_device(&self, info: &DeviceInfo) -> Result<AirspyHfDevice, DeviceError> {
        let device = match (key_serial(&info.key), key_location(&info.key)) {
            (Some(serial), _) => AirspyHf::open_serial(serial),
            // Opening "the first Airspy" would be the wrong radio as soon as two are attached, so
            // a dongle whose descriptor carries no serial is addressed by where it is plugged in.
            (None, Some((bus, address))) => AirspyHf::open_at(bus, address),
            (None, None) => return Err(DeviceError::NotFound(info.id())),
        }
        .map_err(map_err)?;
        Ok(AirspyHfDevice::new(device))
    }
}

impl DeviceDriver for AirspyHfDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        match AirspyHf::list() {
            Ok(found) => found.iter().map(device_info).collect(),
            Err(e) => {
                tracing::warn!("airspy hf+ enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(self.open_device(info)?))
    }
}

struct AirspyHfRadio {
    device: Mutex<AirspyHf>,
}

impl AirspyHfRadio {
    fn lock(&self) -> MutexGuard<'_, AirspyHf> {
        lock(&self.device)
    }
}

impl CaptureRadio for AirspyHfRadio {
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
            tracing::debug!("airspy hf+ stop failed: {e}");
        }
    }
}

pub struct AirspyHfDevice {
    radio: Arc<AirspyHfRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    duplex: Arc<Mutex<DuplexState>>,
    capture: Capture<AirspyHfRadio>,
}

impl AirspyHfDevice {
    fn new(device: AirspyHf) -> Self {
        let capabilities = caps::capabilities(device.sample_rates());
        let settings = caps::settings(device.config());
        tracing::debug!(
            firmware = device.version(),
            serial = ?device.serial(),
            low_if = device.is_low_if(),
            "airspy hf+ ready"
        );
        Self {
            radio: Arc::new(AirspyHfRadio {
                device: Mutex::new(device),
            }),
            duplex: Arc::new(Mutex::new(DuplexState::new(capabilities.duplex))),
            capabilities,
            settings,
            capture: Capture::new(),
        }
    }
}

fn write_to_hardware(
    device: &mut AirspyHf,
    capabilities: &Capabilities,
    delta: &DeviceSettings,
) -> Result<(), DeviceError> {
    if let Some(rate) = delta.sample_rate {
        let rate = u32::try_from(rate.round() as i64)
            .map_err(|_| DeviceError::Unsupported(format!("{rate} Hz is not a sample rate")))?;
        device.set_sample_rate_hz(rate).map_err(map_err)?;
    }
    if let Some(center_hz) = delta.center_hz {
        let hz = u32::try_from(center_hz.round() as i64)
            .map_err(|_| DeviceError::Unsupported(format!("{center_hz} Hz is not a frequency")))?;
        device.set_frequency_hz(hz).map_err(map_err)?;
    }
    for gain in &delta.gains {
        match caps::stage_kind(capabilities, &gain.stage)? {
            GainKind::Amp => device.set_lna(gain.value_db > 0.0).map_err(map_err)?,
            GainKind::Attenuator => device
                .set_attenuation_step(caps::attenuation_step(gain.value_db)?)
                .map_err(map_err)?,
            _ => return Err(DeviceError::Unsupported(format!("no {} stage", gain.stage))),
        }
    }
    if let Some(agc) = &delta.agc {
        let writes = caps::agc_writes(agc)?;
        if let Some(high) = writes.high_threshold {
            device.set_agc_high_threshold(high).map_err(map_err)?;
        }
        device.set_agc(writes.on).map_err(map_err)?;
    }
    if let Some(enabled) = delta.bias_tee {
        device.set_bias_tee(enabled).map_err(map_err)?;
    }
    if let Some(extra) = delta.extra.first() {
        return Err(DeviceError::Unsupported(format!(
            "no {} setting",
            extra.name
        )));
    }
    Ok(())
}

impl SdrDevice for AirspyHfDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        caps::validate(settings, &self.capabilities)?;
        let (result, config) = {
            let mut device = self.radio.lock();
            let result = write_to_hardware(&mut device, &self.capabilities, settings);
            (result, device.config().clone())
        };
        self.settings = caps::settings(&config);
        result
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let sink = single_rx_sink(sinks)?;
        lock(&self.duplex).claim(Direction::Rx)?;
        let started = self.capture.start(
            self.radio.clone(),
            convert::AirspyHfConverter::new(RX_TRANSFER_SIZE / 4),
            sink,
            CaptureConfig::new("sdrmm-airspyhf-rx", DRIVER_ID)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(serial: Option<u64>) -> DeviceDescriptor {
        DeviceDescriptor {
            description: "Airspy HF+",
            serial,
            product_string: Some("AIRSPY HF+".to_string()),
            bus: "020".to_string(),
            address: 7,
        }
    }

    #[test]
    fn a_radio_with_a_serial_is_keyed_by_it() {
        let info = device_info(&descriptor(Some(0x0044_0000_2e19_a5b3)));
        assert_eq!(info.driver, "airspyhf");
        assert_eq!(info.key, "004400002e19a5b3");
        assert_eq!(info.label, "AIRSPY HF+ 004400002e19a5b3");
        assert_eq!(key_serial(&info.key), Some(0x0044_0000_2e19_a5b3));
    }

    #[test]
    fn a_radio_without_a_serial_is_keyed_by_where_it_is_plugged_in() {
        let info = device_info(&descriptor(None));
        assert_eq!(info.key, "noserial-020/7");
        assert_eq!(info.serial, None);
        assert_eq!(key_serial(&info.key), None);
        assert_eq!(key_location(&info.key), Some(("020".to_string(), 7)));
    }

    #[test]
    fn a_serial_key_is_never_read_as_a_location() {
        let key = "004400002e19a5b3";
        assert!(key_location(key).is_none());
    }

    #[test]
    fn a_location_key_that_names_no_address_is_refused() {
        assert_eq!(key_location("noserial-020"), None);
        assert_eq!(key_location("noserial-020/x"), None);
    }
}
