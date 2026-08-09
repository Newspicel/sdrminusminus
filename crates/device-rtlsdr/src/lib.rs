//! `sdrmm-device-rtlsdr` — native RTL-SDR backend (PLAN §6, §15): pure Rust over `nusb` via the
//! vendored `sdrmm-rtl-driver`, so a release artifact ships with no libSoapySDR, no librtlsdr,
//! no C dependency at all. It also exposes what the Soapy path hides for these dongles — the
//! bias tee and the tuner AGC as typed extra settings — and reports the tuner's own gain table
//! instead of a range.
//!
//! What the driver does not program is not advertised: direct sampling, offset tuning and the
//! RTL2832U digital AGC. `apply` rejects those settings rather than accepting them silently.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use caps::{GainMode, Plan};
use convert::IqConverter;
use sdrmm_device::{
    DeviceDriver, DeviceError, Recovery, RestartPolicy, RxSink, SILENT_STREAM_TIMEOUT, SdrDevice,
};
use sdrmm_rtl_driver::{DeviceDescriptor, DeviceDescriptors, DeviceId, RtlSdr, TRANSFER_BUF_SIZE};
use sdrmm_usb_stream::{RxStream, Stopper};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, ExtraValue};

mod caps;
mod convert;

pub(crate) const DRIVER_ID: &str = "rtlsdr";

/// How often the capture loop wakes to re-check its stop flag and the silence clock.
const RECV_POLL: Duration = Duration::from_millis(100);

/// A mutex this crate holds is only ever held across infallible register writes, so a poisoned
/// one carries no half-written state worth refusing — and refusing would mean losing the radio.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Tuning the dongle powers up with. The RTL2832U has no resampler ratio and an untuned PLL
/// after a cold open, so a device that reported that state as its settings would stream
/// nothing usable; these are the values the engine assumes when a device reports none, so
/// hardware, reported settings and engine state agree from the first snapshot.
const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
const DEFAULT_CENTER_HZ: u32 = 100_000_000;

fn map_err(err: sdrmm_rtl_driver::Error) -> DeviceError {
    let text = err.to_string();
    match err {
        sdrmm_rtl_driver::Error::DeviceNotFound => DeviceError::NotFound(text),
        sdrmm_rtl_driver::Error::InvalidSampleRate { .. }
        | sdrmm_rtl_driver::Error::InvalidParam(_) => DeviceError::Unsupported(text),
        _ => DeviceError::Io(text),
    }
}

fn enumerate() -> Result<Vec<DeviceDescriptor>, sdrmm_rtl_driver::Error> {
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
/// The capture thread owns the transport's [`RxStream`]; every setter stays on this side. The
/// `RtlSdr` setters ride the USB *control* endpoint on a clone-backed `nusb::Interface`,
/// independent of the streaming thread's bulk queue, so a retune while streaming costs the
/// sample path nothing and still returns the hardware's real answer.
///
/// The radio is behind a mutex because the capture thread needs it too: an in-place stream
/// restart (tier 1, `PLAN-NATIVE-DRIVERS.md` §2.2) calls `start_streaming` from there. The lock
/// is never held across a blocking read — `apply` takes the same one.
pub struct RtlSdrDevice {
    sdr: Arc<Mutex<RtlSdr>>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// The tuner's discrete gain table in tenths of a dB; `apply` snaps to it.
    gain_table: Vec<i32>,
    running: Arc<AtomicBool>,
    /// Live only while streaming, and replaced by the capture thread on every restart, so
    /// `rx_stop` always reaches the stream that is actually running.
    stopper: Arc<Mutex<Option<Stopper>>>,
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
            sdr: Arc::new(Mutex::new(sdr)),
            capabilities,
            settings,
            gain_table,
            running: Arc::new(AtomicBool::new(false)),
            stopper: Arc::new(Mutex::new(None)),
            worker: None,
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
    // Correction next: it re-tunes from the *cached* centre, so it has to land before a new
    // centre is written, and it also updates the crystal every later tune divides by.
    if let Some(ppm) = plan.ppm {
        sdr.set_freq_correction(ppm).map_err(map_err)?;
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
            let mut sdr = lock(&self.sdr);
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
        if self.worker.is_some() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let stream = lock(&self.sdr).start_streaming().map_err(map_err)?;
        *lock(&self.stopper) = Some(stream.stopper());
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        let sdr = self.sdr.clone();
        let stopper = self.stopper.clone();
        match std::thread::Builder::new()
            .name("sdrmm-rtlsdr-rx".to_string())
            .spawn(move || capture_loop(stream, &sdr, &stopper, &running, sink))
        {
            Ok(worker) => {
                self.worker = Some(worker);
                Ok(())
            }
            Err(e) => {
                // The un-spawned closure drops the stream, which stops the pump; clear the
                // state so a retry is not left looking like a live capture.
                self.running.store(false, Ordering::Release);
                *lock(&self.stopper) = None;
                Err(DeviceError::Io(format!("spawn capture thread: {e}")))
            }
        }
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
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

impl Drop for RtlSdrDevice {
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

/// Blocking capture loop on our own thread, and tier-1 supervisor for its stream.
///
/// Owns the stream, so every exit path drops it, stopping the transfer pump and releasing the
/// bulk endpoint. A stream that ends by itself is restarted in place under [`RestartPolicy`] —
/// measured at 3 ms against the ~9 s a fault, teardown and re-open cost — and only a restart
/// that runs out of attempts reaches `sink.fail` and the engine's destructive fault path.
fn capture_loop(
    mut stream: RxStream,
    sdr: &Mutex<RtlSdr>,
    stopper: &Mutex<Option<Stopper>>,
    running: &AtomicBool,
    mut sink: RxSink,
) {
    let mut converter = IqConverter::with_capacity(TRANSFER_BUF_SIZE / 2);
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
            "rtlsdr stream failed; restarting in place"
        );
        drop(stream);
        std::thread::sleep(delay);
        // A stalled transfer can complete on an odd length, and that half sample's partner is
        // never coming; carried into the fresh stream it would swap I and Q for good.
        converter.reset();
        let restarted = lock(sdr).start_streaming();
        match restarted {
            Ok(fresh) => {
                // Published before the re-check, so a concurrent `rx_stop` either stops this
                // stream or is seen by the check below. One of the two always happens.
                *lock(stopper) = Some(fresh.stopper());
                if !running.load(Ordering::Acquire) {
                    return;
                }
                tracing::info!(attempt, "rtlsdr stream restarted");
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
                sink.push(converter.convert(&block));
                // USB-level loss is a different failure from the engine's ring overruns and
                // must not be silent: a dropped transfer is a gap in the sample stream that no
                // counter downstream can see.
                let total = stream.stats().dropped;
                if total > *dropped {
                    tracing::warn!(dropped = total, "rtlsdr dropped usb transfers");
                    *dropped = total;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // A streaming dongle free-runs and cannot go quiet while healthy, so silence
                // this long is a wedged board — which has no error to report and would
                // otherwise park this thread forever behind a dead waterfall.
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
