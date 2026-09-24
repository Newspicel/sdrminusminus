use std::sync::{Arc, Mutex, MutexGuard};

use caps::{GainMode, Plan};
use driver::{DeviceDescriptor, DeviceDescriptors, RtlSdr};
use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    single_rx_sink,
};
use sdrmm_usb_stream::RxStream;
use sdrmm_wire::{
    AgcGain, AgcSetting, BandwidthSetting, Capabilities, DeviceInfo, DeviceSettings, ExtraValue,
};

mod caps;
mod convert;
mod driver;
mod kraken;

pub use kraken::KrakenDriver;

pub(crate) const DRIVER_ID: &str = "rtlsdr";

const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
pub(crate) const DEFAULT_CENTER_HZ: u32 = 100_000_000;

fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
    if err.is_disconnected() {
        return DeviceError::Disconnected(text);
    }
    if err.is_permission_denied() {
        return DeviceError::PermissionDenied(text);
    }
    if err.is_busy() {
        return DeviceError::InUse(text);
    }
    if err.is_missing() {
        return DeviceError::NotFound(text);
    }
    if err.is_wrong_driver() {
        return DeviceError::Unsupported(format!("{text}: install the WinUSB driver with Zadig"));
    }
    match err {
        driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        driver::Error::InvalidSampleRate { .. }
        | driver::Error::InvalidParam(_)
        | driver::Error::PllLockFailed { .. }
        | driver::Error::UnsupportedTuner(_) => DeviceError::Unsupported(text),
        _ => DeviceError::Io(text),
    }
}

fn enumerate() -> Result<Vec<DeviceDescriptor>, driver::Error> {
    Ok(DeviceDescriptors::new()?.iter().cloned().collect())
}

/// The dongles that are radios in their own right, which is every one that is not a lane of a
/// coherent bank. A bank is opened as the one radio it is, by the driver that knows how.
fn standalone() -> Result<Vec<DeviceDescriptor>, driver::Error> {
    Ok(without_banks(enumerate()?))
}

fn without_banks(attached: Vec<DeviceDescriptor>) -> Vec<DeviceDescriptor> {
    let claimed = kraken::claimed(&attached);
    attached
        .into_iter()
        .enumerate()
        .filter(|(position, _)| !claimed.contains(position))
        .map(|(_, descriptor)| descriptor)
        .collect()
}

#[derive(Default)]
pub struct RtlSdrDriver;

impl RtlSdrDriver {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl DeviceDriver for RtlSdrDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        match standalone() {
            Ok(descriptors) => caps::device_infos(&descriptors),
            Err(e) => {
                tracing::warn!("rtlsdr enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let descriptors =
            standalone().map_err(|e| DeviceError::Io(format!("rtlsdr enumerate: {e}")))?;
        let position = caps::device_infos(&descriptors)
            .iter()
            .position(|probed| probed.key == info.key)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
        let index = descriptors
            .get(position)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?
            .index;
        let sdr = RtlSdr::open(index).map_err(map_err)?;
        Ok(Box::new(RtlSdrDevice::from_sdr(sdr)?))
    }
}

struct RtlRadio {
    sdr: Mutex<RtlSdr>,
}

impl RtlRadio {
    fn lock(&self) -> MutexGuard<'_, RtlSdr> {
        lock(&self.sdr)
    }
}

impl CaptureRadio for RtlRadio {
    type Stream = RxStream;

    fn arm(&self) -> Result<RxStream, DeviceError> {
        self.lock().start_streaming().map_err(map_err)
    }
}

pub struct RtlSdrDevice {
    radio: Arc<RtlRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    gain_table: Vec<i32>,
    capture: Capture<RtlRadio>,
}

impl RtlSdrDevice {
    fn from_sdr(mut sdr: RtlSdr) -> Result<Self, DeviceError> {
        let capabilities = caps::capabilities(sdr.board_variant(), sdr.gains());
        let gain_table = sdr.gains().to_vec();
        tracing::info!(
            tuner = ?sdr.tuner_type(),
            board = ?sdr.board_variant(),
            gain_steps = gain_table.len(),
            "opened rtlsdr device"
        );

        sdr.set_sample_rate(DEFAULT_SAMPLE_RATE_HZ)
            .map_err(map_err)?;
        sdr.set_center_freq(DEFAULT_CENTER_HZ).map_err(map_err)?;
        sdr.set_gain_auto().map_err(map_err)?;
        let bias_tee = sdr.bias_t_at_startup();
        sdr.set_bias_t(bias_tee).map_err(map_err)?;

        let extra = (!sdr.board_variant().upconverts_hf())
            .then(|| ExtraValue {
                name: caps::DIRECT_SAMPLING.to_string(),
                value: sdr.direct_sampling().as_str().into(),
            })
            .into_iter()
            .collect();

        let settings = DeviceSettings {
            center_hz: Some(f64::from(sdr.center_freq())),
            sample_rate: Some(f64::from(sdr.sample_rate())),
            ppm: Some(f64::from(sdr.freq_correction())),
            antenna: Some("RX".to_string()),
            bandwidth: Some(BandwidthSetting::Auto),
            bias_tee: Some(bias_tee),
            agc: Some(AgcSetting::switched(true)),
            extra,
            ..DeviceSettings::default()
        };

        Ok(Self {
            radio: Arc::new(RtlRadio {
                sdr: Mutex::new(sdr),
            }),
            capabilities,
            settings,
            gain_table,
            capture: Capture::new(),
        })
    }
}

fn apply_to_hardware(sdr: &mut RtlSdr, plan: &Plan) -> Result<(), DeviceError> {
    if let Some(mode) = plan.direct_sampling {
        sdr.set_direct_sampling(mode).map_err(map_err)?;
    }
    if let Some(rate) = plan.sample_rate {
        sdr.set_sample_rate(rate).map_err(map_err)?;
    }
    if let Some(hz) = plan.center_hz {
        sdr.set_center_freq(hz).map_err(map_err)?;
    }
    if let Some(bw) = plan.bandwidth {
        sdr.set_bandwidth(bw).map_err(map_err)?;
        let center = sdr.center_freq();
        sdr.set_center_freq(center).map_err(map_err)?;
    }
    if let Some(ppm) = plan.ppm {
        sdr.set_freq_correction(ppm).map_err(map_err)?;
    }
    match plan.gain {
        Some(GainMode::Auto) => sdr.set_gain_auto().map_err(map_err)?,
        Some(GainMode::Manual(tenths)) => sdr.set_gain_manual(tenths).map_err(map_err)?,
        None => {}
    }
    if let Some(on) = plan.bias_tee {
        sdr.set_bias_t(on).map_err(map_err)?;
    }
    Ok(())
}

impl SdrDevice for RtlSdrDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let plan = caps::validate(
            settings,
            &self.capabilities,
            &self.settings,
            &self.gain_table,
        )?;
        let (result, center_hz, sample_rate, ppm) = {
            let mut sdr = self.radio.lock();
            let result = apply_to_hardware(&mut sdr, &plan);
            (
                result,
                sdr.center_freq(),
                sdr.sample_rate(),
                sdr.freq_correction(),
            )
        };
        self.settings.center_hz = Some(f64::from(center_hz));
        self.settings.sample_rate = Some(f64::from(sample_rate));
        self.settings.ppm = Some(f64::from(ppm));
        result?;
        self.settings.merge_from(&plan.applied);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            convert::converter(),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-rtlsdr-rx", DRIVER_ID)
                .with_sample_rate(self.settings.sample_rate),
        )
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
    }

    fn agc_gains(&self) -> Result<Vec<AgcGain>, DeviceError> {
        if !self.settings.agc.as_ref().is_some_and(|agc| agc.on) {
            return Ok(Vec::new());
        }
        let tenths = self.radio.lock().tuner_gain().map_err(map_err)?;
        Ok(vec![AgcGain {
            stream: 0,
            value_db: f64::from(tenths) / 10.0,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::BoardVariant;

    fn dongle(serial: &str, port: u8, hub: Option<u8>) -> DeviceDescriptor {
        DeviceDescriptor {
            index: usize::from(port),
            bus: "001".to_string(),
            address: port,
            manufacturer: None,
            product: None,
            serial: Some(serial.to_string()),
            port_chain: hub.map_or_else(|| vec![port], |hub| vec![hub, port]),
            board_variant: BoardVariant::Generic,
        }
    }

    #[test]
    fn a_banks_dongles_are_not_offered_as_radios_of_their_own() {
        let mut attached: Vec<DeviceDescriptor> = (0..5)
            .map(|lane| dongle(&(1000 + lane).to_string(), lane as u8 + 1, Some(4)))
            .collect();
        attached.push(dongle("00000123", 9, None));
        let standalone = without_banks(attached);
        assert_eq!(standalone.len(), 1);
        assert_eq!(standalone[0].serial.as_deref(), Some("00000123"));
    }

    #[test]
    fn ordinary_dongles_are_all_offered() {
        let attached = vec![dongle("00000123", 1, None), dongle("00000124", 2, None)];
        assert_eq!(without_banks(attached).len(), 2);
    }

    #[test]
    fn driver_id_is_the_wire_id() {
        assert_eq!(RtlSdrDriver::new().id(), "rtlsdr");
    }

    fn control_failure(source: nusb::transfer::TransferError) -> driver::Error {
        driver::Error::ControlTransfer {
            op: "demod write of page 0x1:0x0001".to_string(),
            source,
        }
    }

    #[test]
    fn an_unplugged_dongle_reads_as_gone_rather_than_as_a_transfer_that_failed() {
        assert!(matches!(
            map_err(control_failure(nusb::transfer::TransferError::Disconnected)),
            DeviceError::Disconnected(_)
        ));
        assert!(matches!(
            map_err(control_failure(nusb::transfer::TransferError::Stall)),
            DeviceError::Io(_)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_dongle_that_stopped_answering_the_bus_reads_as_gone() {
        assert!(matches!(
            map_err(control_failure(nusb::transfer::TransferError::Unknown(
                0xe000_02ed
            ))),
            DeviceError::Disconnected(_)
        ));
    }
}
