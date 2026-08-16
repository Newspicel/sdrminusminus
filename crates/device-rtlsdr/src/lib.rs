use std::sync::{Arc, Mutex, MutexGuard};

use caps::{GainMode, Plan};
use driver::{BoardVariant, DeviceDescriptor, DeviceDescriptors, RtlSdr};
use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    single_rx_sink,
};
use sdrmm_usb_stream::RxStream;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, ExtraValue};

mod caps;
mod convert;
mod driver;

pub(crate) const DRIVER_ID: &str = "rtlsdr";

const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
pub(crate) const DEFAULT_CENTER_HZ: u32 = 100_000_000;

fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
    if err.is_disconnected() {
        return DeviceError::Disconnected(text);
    }
    match err {
        driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        driver::Error::InvalidSampleRate { .. } | driver::Error::InvalidParam(_) => {
            DeviceError::Unsupported(text)
        }
        _ => DeviceError::Io(text),
    }
}

fn enumerate() -> Result<Vec<DeviceDescriptor>, driver::Error> {
    Ok(DeviceDescriptors::new()?.iter().cloned().collect())
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
        match enumerate() {
            Ok(descriptors) => caps::device_infos(&descriptors),
            Err(e) => {
                tracing::warn!("rtlsdr enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let descriptors =
            enumerate().map_err(|e| DeviceError::Io(format!("rtlsdr enumerate: {e}")))?;
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
        sdr.set_bias_t(false).map_err(map_err)?;

        let mut extra = vec![
            ExtraValue {
                name: caps::BIAS_TEE.to_string(),
                value: false.into(),
            },
            ExtraValue {
                name: caps::AGC.to_string(),
                value: true.into(),
            },
        ];
        if sdr.board_variant() != BoardVariant::RtlSdrBlogV4 {
            extra.push(ExtraValue {
                name: caps::DIRECT_SAMPLING.to_string(),
                value: sdr.direct_sampling().as_str().into(),
            });
        }

        let settings = DeviceSettings {
            center_hz: Some(f64::from(sdr.center_freq())),
            sample_rate: Some(f64::from(sdr.sample_rate())),
            ppm: Some(f64::from(sdr.freq_correction())),
            antenna: Some("RX".to_string()),
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
        // No retune here: `caps::validate` always plans a centre alongside a mode change, and the
        // unconditional write below lands it once the sample rate is in place.
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
        if plan.clear_bandwidth {
            self.settings.bandwidth = None;
        }
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            convert::converter(),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-rtlsdr-rx", DRIVER_ID),
        )
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_id_is_the_wire_id() {
        assert_eq!(RtlSdrDriver::new().id(), "rtlsdr");
    }

    #[test]
    fn an_unplugged_dongle_reads_as_gone_rather_than_as_a_transfer_that_failed() {
        assert!(matches!(
            map_err(driver::Error::ControlTransfer(
                nusb::transfer::TransferError::Disconnected
            )),
            DeviceError::Disconnected(_)
        ));
        assert!(matches!(
            map_err(driver::Error::ControlTransfer(
                nusb::transfer::TransferError::Stall
            )),
            DeviceError::Io(_)
        ));
    }
}
