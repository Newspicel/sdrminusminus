//! `sdrmm-device-rtlsdr` — native RTL-SDR backend (PLAN §6, §15): the RTL2832U driver and the
//! `SdrDevice` implementation over it, pure Rust on `nusb`, so a release artifact ships with no
//! libSoapySDR, no librtlsdr, no C dependency at all. It exposes what the Soapy path hides for
//! these dongles — the bias tee, the tuner AGC and crystal correction as typed settings — and
//! reports the tuner's own gain table instead of a range.
//!
//! Four layers, in dependency order:
//!
//! - [`driver`] — the radio: enumeration, registers, I2C, the R82xx tuner. No wire types.
//! - `convert` — the table that turns the RTL2832U's unsigned 8-bit codes into `cf32`.
//! - `caps` — the pure translation to the wire capability model, and `apply`'s validation.
//! - this module — `DeviceDriver`/`SdrDevice` over `sdrmm-device`'s shared capture machinery.
//!
//! Streaming lives in `sdrmm-usb-stream` and supervision in `sdrmm-device`, both shared with the
//! HackRF backend: the transfer queue, the USB error policy and the restart loop are what the
//! two radios genuinely have in common, and getting them wrong on each side separately is the
//! defect this driver exists to fix (PLAN §17). What is left here is the RTL-SDR itself.
//!
//! What the driver does not program is not advertised: direct sampling, offset tuning and the
//! RTL2832U digital AGC. `apply` rejects those settings rather than accepting them silently.

use std::sync::{Arc, Mutex, MutexGuard};

use caps::{GainMode, Plan};
use driver::{DeviceDescriptor, DeviceDescriptors, RtlSdr};
use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
};
use sdrmm_usb_stream::RxStream;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, ExtraValue};

mod caps;
mod convert;
mod driver;

pub(crate) const DRIVER_ID: &str = "rtlsdr";

/// Tuning the dongle powers up with. The RTL2832U has no resampler ratio and an untuned PLL
/// after a cold open, so a device that reported that state as its settings would stream
/// nothing usable; these are the values the engine assumes when a device reports none, so
/// hardware, reported settings and engine state agree from the first snapshot.
const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
const DEFAULT_CENTER_HZ: u32 = 100_000_000;

fn map_err(err: driver::Error) -> DeviceError {
    let text = err.to_string();
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

/// Driver for RTL2832U dongles with an R820T or R828D tuner (including the RTL-SDR Blog V4).
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
                // probe() cannot return errors; an enumerate failure must not pass as a
                // silent "no devices".
                tracing::warn!("rtlsdr enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        // Re-enumerate and re-derive the keys rather than trusting the caller's: the key may be
        // a serial or a bus/address pair, and only `caps::device_infos` decides which (a serial
        // shared by two dongles identifies neither).
        let descriptors =
            enumerate().map_err(|e| DeviceError::Io(format!("rtlsdr enumerate: {e}")))?;
        let position = caps::device_infos(&descriptors)
            .iter()
            .position(|probed| probed.key == info.key)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
        // `DeviceDescriptor::index` is the position in the same filtered enumeration, so it
        // selects this descriptor and not another dongle that moved into its slot.
        let index = descriptors
            .get(position)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?
            .index;
        let sdr = RtlSdr::open(index).map_err(map_err)?;
        Ok(Box::new(RtlSdrDevice::from_sdr(sdr)?))
    }
}

/// The radio, as the shared capture supervisor sees it.
///
/// Behind a mutex because both threads need it: the control thread retunes through it while the
/// capture thread re-arms the stream from it. The lock is never held across a blocking read —
/// the `RtlSdr` setters ride the USB *control* endpoint on a clone-backed `nusb::Interface`,
/// independent of the streaming thread's bulk queue, so a retune while streaming costs the
/// sample path nothing and still returns the hardware's real answer.
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

    /// `start_streaming` resets the endpoint's FIFO before it queues anything, so the same call
    /// serves a cold start and an in-place restart. There is nothing to disarm afterwards: the
    /// dongle stops producing when the transfers stop being submitted.
    fn arm(&self) -> Result<RxStream, DeviceError> {
        self.lock().start_streaming().map_err(map_err)
    }
}

/// An opened RTL-SDR receiver.
pub struct RtlSdrDevice {
    radio: Arc<RtlRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// The tuner's discrete gain table in tenths of a dB; `apply` snaps to it.
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
        // GPIO0 keeps its previous level across a re-open, so phantom power must be driven
        // low explicitly — a bias tee left on can damage whatever is on the antenna port.
        // A dongle whose EEPROM forces it on ignores this (`RtlSdr::set_bias_t`).
        sdr.set_bias_t(false).map_err(map_err)?;

        let settings = DeviceSettings {
            center_hz: Some(f64::from(sdr.center_freq())),
            sample_rate: Some(f64::from(sdr.sample_rate())),
            ppm: Some(f64::from(sdr.freq_correction())),
            antenna: Some("RX".to_string()),
            extra: vec![
                ExtraValue {
                    name: caps::BIAS_TEE.to_string(),
                    value: false.into(),
                },
                ExtraValue {
                    name: caps::AGC.to_string(),
                    value: true.into(),
                },
            ],
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
    // Rate first: `set_sample_rate` re-tunes the tuner and recomputes its filter from the new
    // rate (the driver mirrors librtlsdr here), so a center or bandwidth written before it
    // would be overwritten.
    if let Some(rate) = plan.sample_rate {
        sdr.set_sample_rate(rate).map_err(map_err)?;
    }
    if let Some(hz) = plan.center_hz {
        sdr.set_center_freq(hz).map_err(map_err)?;
    }
    if let Some(bw) = plan.bandwidth {
        sdr.set_bandwidth(bw).map_err(map_err)?;
        // A width change reassigns the tuner's IF and rewrites only the demodulator's IF
        // register — the tuner PLL still sits at `centre + old_IF`, so the radio quietly
        // receives `centre + (old_IF - new_IF)` while every consumer is told `centre`.
        // librtlsdr's `r820t_set_bw` ends with a re-tune for exactly this reason, and the
        // driver keeps that re-tune in `set_sample_rate` but not here. Re-programming the PLL
        // from the cached centre closes the loop.
        let center = sdr.center_freq();
        sdr.set_center_freq(center).map_err(map_err)?;
    }
    // Correction last of the tuning writes: it re-tunes from the *cached* centre, so applying
    // it first would park the PLL on the centre the caller is replacing before moving it again.
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
        // The driver caches the values it last wrote successfully — including the resampler's
        // *actual* rate, which integer division can move off the request — so these two
        // getters are the device's own truth. Resync them either way: a mid-batch failure
        // leaves the hardware partially retuned, and the control plane must not keep
        // reporting pre-batch values (the Soapy backend resyncs for the same reason).
        self.settings.center_hz = Some(f64::from(center_hz));
        self.settings.sample_rate = Some(f64::from(sample_rate));
        self.settings.ppm = Some(f64::from(ppm));
        result?;
        self.settings.merge_from(&plan.applied);
        // `merge_from` cannot clear a field, and an automatic filter width is the absence of
        // one (as the Soapy backend reports it).
        if plan.bandwidth == Some(0) {
            self.settings.bandwidth = None;
        }
        Ok(())
    }

    fn rx_start(&mut self, sink: RxSink) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            convert::converter(),
            sink,
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

    // No test may call `RtlSdrDriver::probe`/`open`, or anything else on `RtlSdrDevice`: both
    // walk the USB bus, and what they find is whatever is plugged into the machine running
    // them (PLAN §14: no hardware in CI, ever). Everything testable is a pure function in
    // `caps` or `convert` — that is why the mapping, validation and sample conversion live
    // there and not inline here.
}
