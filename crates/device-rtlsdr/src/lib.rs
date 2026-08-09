//! `sdrmm-device-rtlsdr` — native RTL-SDR backend (PLAN §6, §15): pure Rust over `nusb`, so a
//! release artifact ships with no libSoapySDR, no librtlsdr, no C dependency at all. It also
//! exposes what the Soapy path hides for these dongles — the bias tee and the tuner AGC as
//! typed extra settings — and reports the tuner's own gain table instead of a range.
//!
//! What rs-rtl 0.4.2 cannot reach is not advertised: direct sampling, crystal (ppm) correction,
//! offset tuning and the RTL2832U digital AGC have no public API, and the low-level register
//! layer of an *opened* device is not reachable either (`RtlSdr` owns its `Device` privately).
//! `apply` rejects those settings rather than accepting them silently.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use caps::{GainMode, Plan};
use convert::IqConverter;
use rs_rtl::{
    AsyncReadControlHandle, AsyncReadHandle, DeviceDescriptor, DeviceDescriptors, DeviceId, RtlSdr,
    TRANSFER_BUF_SIZE,
};
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, SdrDevice};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, ExtraValue};

mod caps;
mod convert;

pub(crate) const DRIVER_ID: &str = "rtlsdr";

/// Tuning the dongle powers up with. The RTL2832U has no resampler ratio and an untuned PLL
/// after a cold open, so a device that reported that state as its settings would stream
/// nothing usable; these are the values the engine assumes when a device reports none, so
/// hardware, reported settings and engine state agree from the first snapshot.
const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
const DEFAULT_CENTER_HZ: u32 = 100_000_000;

fn map_err(err: rs_rtl::Error) -> DeviceError {
    let text = err.to_string();
    match err {
        rs_rtl::Error::DeviceNotFound => DeviceError::NotFound(text),
        rs_rtl::Error::InvalidSampleRate { .. } | rs_rtl::Error::InvalidParam(_) => {
            DeviceError::Unsupported(text)
        }
        rs_rtl::Error::AlreadyStreaming => DeviceError::AlreadyStreaming,
        _ => DeviceError::Io(text),
    }
}

fn enumerate() -> Result<Vec<DeviceDescriptor>, rs_rtl::Error> {
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
        let sdr = RtlSdr::open(DeviceId::Index(index)).map_err(map_err)?;
        Ok(Box::new(RtlSdrDevice::from_sdr(sdr)?))
    }
}

/// An opened RTL-SDR receiver.
///
/// The capture thread owns rs-rtl's read handle; every setter stays on this side. Retunes while
/// streaming deliberately do *not* go through [`AsyncReadControlHandle`]: its commands are
/// fire-and-forget (a failed tune reaches only rs-rtl's own `warn!`, and the call returns `Ok`
/// as soon as the command is queued), which would make a rejected retune look applied. The
/// `RtlSdr` setters return the real result, and they ride the USB *control* endpoint on a
/// clone-backed `nusb::Interface`, independent of the streaming thread's bulk queue. Because we
/// never send control commands, the tuner-register shadow that rs-rtl clones into the streaming
/// thread stays unused and cannot drift from ours.
pub struct RtlSdrDevice {
    sdr: RtlSdr,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// The tuner's discrete gain table in tenths of a dB; `apply` snaps to it.
    gain_table: Vec<i32>,
    running: Arc<AtomicBool>,
    /// Live only while streaming: the one way to reach rs-rtl's streaming thread once the read
    /// handle has moved to the capture thread.
    control: Option<AsyncReadControlHandle>,
    worker: Option<JoinHandle<()>>,
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
        // A dongle whose EEPROM forces it on ignores this (rs-rtl `set_bias_t`).
        sdr.set_bias_t(false).map_err(map_err)?;

        let settings = DeviceSettings {
            center_hz: Some(f64::from(sdr.center_freq())),
            sample_rate: Some(f64::from(sdr.sample_rate())),
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
            sdr,
            capabilities,
            settings,
            gain_table,
            running: Arc::new(AtomicBool::new(false)),
            control: None,
            worker: None,
        })
    }

    fn apply_to_hardware(&mut self, plan: &Plan) -> Result<(), DeviceError> {
        // Rate first: `set_sample_rate` re-tunes the tuner and recomputes its filter from the
        // new rate (rs-rtl mirrors librtlsdr here), so a center or bandwidth written before it
        // would be overwritten.
        if let Some(rate) = plan.sample_rate {
            self.sdr.set_sample_rate(rate).map_err(map_err)?;
        }
        if let Some(hz) = plan.center_hz {
            self.sdr.set_center_freq(hz).map_err(map_err)?;
        }
        if let Some(bw) = plan.bandwidth {
            self.sdr.set_bandwidth(bw).map_err(map_err)?;
        }
        match plan.gain {
            Some(GainMode::Auto) => self.sdr.set_gain_auto().map_err(map_err)?,
            Some(GainMode::Manual(tenths)) => {
                self.sdr.set_gain_manual(tenths).map_err(map_err)?;
            }
            None => {}
        }
        if let Some(on) = plan.bias_tee {
            self.sdr.set_bias_t(on).map_err(map_err)?;
        }
        Ok(())
    }
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
        let result = self.apply_to_hardware(&plan);
        // rs-rtl caches the values it last wrote successfully — including the resampler's
        // *actual* rate, which integer division can move off the request — so these two
        // getters are the device's own truth. Resync them either way: a mid-batch failure
        // leaves the hardware partially retuned, and the control plane must not keep
        // reporting pre-batch values (the Soapy backend resyncs for the same reason).
        self.settings.center_hz = Some(f64::from(self.sdr.center_freq()));
        self.settings.sample_rate = Some(f64::from(self.sdr.sample_rate()));
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
        if self.worker.is_some() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let handle = self.sdr.start_streaming().map_err(map_err)?;
        let control = handle.control_handle();
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        let worker = std::thread::Builder::new()
            .name("sdrmm-rtlsdr-rx".to_string())
            .spawn(move || capture_loop(&handle, &running, sink))
            .map_err(|e| DeviceError::Io(format!("spawn capture thread: {e}")))?;
        self.control = Some(control);
        self.worker = Some(worker);
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
        // The capture thread is parked in a blocking `recv`, so the stop has to come from the
        // far end: rs-rtl's streaming thread closes the sample channel on its way out, which
        // is what releases that `recv`.
        if let Some(control) = self.control.take() {
            control.stop();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RtlSdrDevice {
    fn drop(&mut self) {
        self.rx_stop();
    }
}

/// Blocking capture loop on our own thread. Owns rs-rtl's read handle, so dropping it on any
/// exit path stops the streaming thread and releases the bulk endpoint.
fn capture_loop(handle: &AsyncReadHandle, running: &AtomicBool, mut sink: RxSink) {
    let mut converter = IqConverter::with_capacity(TRANSFER_BUF_SIZE / 2);
    let mut dropped = 0u64;
    while running.load(Ordering::Acquire) {
        let Some(block) = handle.recv() else {
            // rs-rtl closes the channel when its streaming thread gives up on the endpoint
            // (five consecutive transfer errors = unplugged) or when it was told to stop.
            // `running` still set means nobody asked, so the dongle is gone.
            if running.load(Ordering::Acquire) {
                sink.fail(DeviceError::Io("device lost: usb stream ended".to_string()));
            }
            break;
        };
        sink.push(converter.convert(&block));
        // USB-level loss is a different failure from the engine's ring overruns and must not
        // be silent. rs-rtl 0.4.2 hands samples over with a blocking send, so this counter
        // only moves if a later version adopts a drop policy — surface it if it ever does.
        let total = handle.dropped_chunks();
        if total > dropped {
            tracing::warn!(dropped = total, "rtlsdr dropped usb chunks");
            dropped = total;
        }
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
