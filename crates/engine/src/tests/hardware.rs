use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_device::{DeviceDriver, DeviceError, RxSink};
use sdrmm_wire::{DeviceInfo, DeviceSettings, GainValue};

const CENTER_HZ: f64 = 100_000_000.0;
// Tried in order; the first that both drivers accept is what a comparison runs at. A HackRF
// takes any rate in 2..20 MHz through its own driver but only a discrete menu through Soapy.
const RATE_CANDIDATES: [f64; 5] = [2_048_000.0, 2_400_000.0, 8_000_000.0, 10_000_000.0, 20e6];
const GAIN_DB: f64 = 20.0;
const CAPTURE: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);
const SWEEP_CAPTURE: Duration = Duration::from_millis(1200);
const CLIPPING_PEAK: f64 = 0.98;

#[derive(Debug)]
struct Measurement {
    samples: usize,
    elapsed: Duration,
    mean_i: f64,
    mean_q: f64,
    rms: f64,
    peak: f64,
}

impl Measurement {
    fn effective_rate(&self) -> f64 {
        self.samples as f64 / self.elapsed.as_secs_f64()
    }

    fn clipping(&self) -> bool {
        self.peak >= CLIPPING_PEAK
    }
}

#[derive(Default)]
struct Accumulator {
    samples: usize,
    sum_i: f64,
    sum_q: f64,
    sum_power: f64,
    peak: f64,
}

impl Accumulator {
    fn push(&mut self, samples: &[Complex<f32>]) {
        self.samples += samples.len();
        for sample in samples {
            let (i, q) = (f64::from(sample.re), f64::from(sample.im));
            self.sum_i += i;
            self.sum_q += q;
            let power = i * i + q * q;
            self.sum_power += power;
            self.peak = self.peak.max(power.sqrt());
        }
    }

    fn finish(self, elapsed: Duration) -> Measurement {
        let n = self.samples.max(1) as f64;
        Measurement {
            samples: self.samples,
            elapsed,
            mean_i: self.sum_i / n,
            mean_q: self.sum_q / n,
            rms: (self.sum_power / n).sqrt(),
            peak: self.peak,
        }
    }
}

/// Radios are exclusive: two of these tests running on cargo's default thread pool would fight
/// over the same devices and fail on whichever lost the claim.
static HARDWARE: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    HARDWARE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(f64::MIN_POSITIVE).log10()
}

fn render(gains: &[GainValue]) -> String {
    gains
        .iter()
        .map(|gain| format!("{}={}", gain.stage, gain.value_db))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every advertised stage set to `gain_db`, which each driver then snaps to whatever its hardware
/// can produce.
fn every_stage(driver: &dyn DeviceDriver, info: &DeviceInfo, gain_db: f64) -> Vec<GainValue> {
    let device = driver
        .open(info)
        .unwrap_or_else(|e| panic!("{} open for gain stages: {e}", driver.id()));
    device
        .capabilities()
        .gains
        .iter()
        .map(|stage| GainValue {
            stage: stage.name.clone(),
            value_db: gain_db.clamp(stage.range.min, stage.range.max),
        })
        .collect()
}

fn accepts(capabilities: &sdrmm_wire::Capabilities, rate: f64) -> bool {
    capabilities
        .sample_rate_range
        .is_some_and(|range| range.min <= rate && rate <= range.max)
        || capabilities
            .sample_rates
            .iter()
            .any(|have| (have - rate).abs() <= rate * 0.001)
}

/// The first candidate rate both radios will hold, so a comparison is never decided by one of
/// them silently substituting a rate the other refused.
fn common_rate(
    a: &dyn DeviceDriver,
    ai: &DeviceInfo,
    b: &dyn DeviceDriver,
    bi: &DeviceInfo,
) -> f64 {
    let capabilities = |driver: &dyn DeviceDriver, info: &DeviceInfo| {
        driver
            .open(info)
            .map(|device| device.capabilities().clone())
            .ok()
    };
    let (Some(left), Some(right)) = (capabilities(a, ai), capabilities(b, bi)) else {
        return RATE_CANDIDATES[0];
    };
    RATE_CANDIDATES
        .into_iter()
        .find(|rate| accepts(&left, *rate) && accepts(&right, *rate))
        .unwrap_or(RATE_CANDIDATES[0])
}

fn measure(
    driver: &dyn DeviceDriver,
    info: &DeviceInfo,
    gains: &[GainValue],
    rate: f64,
    capture: Duration,
) -> Result<(Measurement, DeviceSettings), DeviceError> {
    let mut device = driver.open(info)?;
    device.apply(&DeviceSettings {
        center_hz: Some(CENTER_HZ),
        sample_rate: Some(rate),
        gains: gains.to_vec(),
        ..DeviceSettings::default()
    })?;
    let applied = device.settings().clone();

    let accumulator = Arc::new(Mutex::new(Accumulator::default()));
    let recording = Arc::new(AtomicUsize::new(0));
    let sink_accumulator = accumulator.clone();
    let gate = recording.clone();
    device.rx_start(vec![RxSink::new(move |samples| {
        if gate.load(Ordering::Acquire) == 1 {
            sink_accumulator
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(samples);
        }
    })])?;

    std::thread::sleep(SETTLE);
    recording.store(1, Ordering::Release);
    let started = Instant::now();
    std::thread::sleep(capture);
    recording.store(0, Ordering::Release);
    let elapsed = started.elapsed();
    device.rx_stop();
    drop(device);

    let accumulator = Arc::try_unwrap(accumulator)
        .map_err(|_| DeviceError::Io("the sink outlived the capture".to_string()))?
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok((accumulator.finish(elapsed), applied))
}

fn native_drivers() -> Vec<Box<dyn DeviceDriver>> {
    vec![
        Box::new(sdrmm_device_rtlsdr::RtlSdrDriver::new()),
        Box::new(sdrmm_device_hackrf::HackRfDriver::new()),
    ]
}

/// Soapy with nothing hidden, which is the only way to reach the modules a normal build excludes
/// precisely because these drivers replace them.
fn soapy_peer(info: &DeviceInfo) -> Option<(sdrmm_device_soapy::SoapyDriver, DeviceInfo)> {
    let soapy = sdrmm_device_soapy::SoapyDriver::new();
    let peer = soapy
        .probe()
        .into_iter()
        .find(|candidate| candidate.serial.as_ref() == info.serial.as_ref())?;
    Some((soapy, peer))
}

#[test]
#[ignore = "needs an RTL-SDR or HackRF on the bus"]
fn a_native_driver_streams_what_soapy_streams_from_the_same_radio() {
    let _guard = exclusive();
    let mut compared = 0;
    for native in native_drivers() {
        for info in native.probe() {
            let serial = info.serial.clone().unwrap_or_else(|| info.key.clone());
            let Some((soapy, peer)) = soapy_peer(&info) else {
                println!("{:>7} {serial}: soapy does not list it", native.id());
                continue;
            };
            let rate = common_rate(native.as_ref(), &info, &soapy, &peer);
            let asked = every_stage(native.as_ref(), &info, GAIN_DB);
            let (native_measurement, native_applied) =
                measure(native.as_ref(), &info, &asked, rate, CAPTURE)
                    .unwrap_or_else(|e| panic!("{} capture: {e}", native.id()));

            assert!(
                native_measurement.samples > 0,
                "{} delivered nothing",
                native.id()
            );
            let delivered = native_measurement.effective_rate();
            assert!(
                (delivered - rate).abs() / rate < 0.02,
                "{} delivered {delivered:.1} Hz against {rate} Hz",
                native.id()
            );
            // Soapy is asked for exactly what the native driver reported programming, so the
            // comparison is of two signal paths at one hardware setting rather than of two
            // opinions about which gain-table entry an off-table request should take.
            let (soapy_measurement, soapy_applied) =
                measure(&soapy, &peer, &native_applied.gains, rate, CAPTURE)
                    .unwrap_or_else(|e| panic!("soapy capture: {e}"));

            let offset_db = db(native_measurement.rms / soapy_measurement.rms);
            println!(
                "{:>7} {serial} at {rate} Hz\n  native rms {:.5} peak {:.5} dc ({:+.5}, {:+.5}) gains [{}]\n   soapy rms {:.5} peak {:.5} dc ({:+.5}, {:+.5}) gains [{}]\n  offset {:+.2} dB",
                native.id(),
                native_measurement.rms,
                native_measurement.peak,
                native_measurement.mean_i,
                native_measurement.mean_q,
                render(&native_applied.gains),
                soapy_measurement.rms,
                soapy_measurement.peak,
                soapy_measurement.mean_i,
                soapy_measurement.mean_q,
                render(&soapy_applied.gains),
                offset_db,
            );

            // A subset rather than an equality: SoapyHackRF presents the switched +14 dB RF amp as
            // a third gain stage, where the native driver presents the switch it actually is as
            // the `amp` boolean. Every stage both drivers do model has to agree.
            for gain in &native_applied.gains {
                let held = soapy_applied
                    .gains
                    .iter()
                    .find(|candidate| candidate.stage == gain.stage);
                assert_eq!(
                    held.map(|held| held.value_db),
                    Some(gain.value_db),
                    "soapy did not hold {} at what the native driver reported",
                    gain.stage
                );
            }
            assert!(
                offset_db.abs() < 1.5,
                "{} is {offset_db:+.2} dB from soapy on the same antenna at the same gain",
                native.id()
            );
            assert!(
                (native_measurement.mean_i - soapy_measurement.mean_i).abs() < 0.01
                    && (native_measurement.mean_q - soapy_measurement.mean_q).abs() < 0.01,
                "{} DC ({:+.5}, {:+.5}) against soapy DC ({:+.5}, {:+.5}) — the conversion tables \
                 disagree about where zero is",
                native.id(),
                native_measurement.mean_i,
                native_measurement.mean_q,
                soapy_measurement.mean_i,
                soapy_measurement.mean_q
            );
            compared += 1;
        }
    }
    assert!(compared > 0, "no radio was comparable against soapy");
}

#[test]
#[ignore = "needs an RTL-SDR or HackRF on the bus"]
fn the_two_drivers_track_each_other_across_the_gain_range() {
    let _guard = exclusive();
    let mut steps = 0;
    for native in native_drivers() {
        for info in native.probe() {
            let serial = info.serial.clone().unwrap_or_else(|| info.key.clone());
            let Some((soapy, peer)) = soapy_peer(&info) else {
                continue;
            };
            let rate = common_rate(native.as_ref(), &info, &soapy, &peer);
            println!("\n=== {} {serial} at {rate} Hz ===", native.id());
            println!(
                "{:>6}  {:>20} {:>9}  {:>9}  {:>8}  {:>7}",
                "ask", "gains", "native", "soapy", "delta dB", "clipped"
            );
            for gain_db in [0.0, 8.0, 16.0, 20.0, 26.0, 32.0, 40.0, 50.0] {
                let asked = every_stage(native.as_ref(), &info, gain_db);
                let (n, na) = measure(native.as_ref(), &info, &asked, rate, SWEEP_CAPTURE)
                    .unwrap_or_else(|e| panic!("{} at {gain_db} dB: {e}", native.id()));
                let (s, _) = measure(&soapy, &peer, &na.gains, rate, SWEEP_CAPTURE)
                    .unwrap_or_else(|e| panic!("soapy at {gain_db} dB: {e}"));
                let clipped = n.clipping() || s.clipping();
                let offset_db = db(n.rms / s.rms);
                println!(
                    "{gain_db:>6.1}  {:>20} {:>9.5}  {:>9.5}  {offset_db:>+8.2}  {clipped:>7}",
                    render(&na.gains),
                    n.rms,
                    s.rms,
                );
                // Only below compression: at the top of the range the front end is non-linear and
                // a fraction of a dB of difference between two captures is the radio, not the
                // driver.
                if !clipped {
                    assert!(
                        offset_db.abs() < 1.5,
                        "{} is {offset_db:+.2} dB from soapy at [{}]",
                        native.id(),
                        render(&na.gains)
                    );
                }
                steps += 1;
            }
        }
    }
    assert!(steps > 0, "no radio was swept against soapy");
}
