use std::{
    f64::consts::TAU,
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::RingBuffer;
use sdrmm_dsp::{
    array_sync::FastConvolver,
    fft::FftPair,
    manifold::{Direction, Vec3, steer},
};
use sdrmm_wire::{CalSourceKind, LaneKey, LaneSolution};

use super::*;
use crate::array::capture::{CaptureRequest, CaptureStart};

const RATE: f64 = 2_400_000.0;
const CENTER: f64 = 433.92e6;
const BAND: f64 = 0.4;

struct Gaussian(u64);

impl Gaussian {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        (bits as f64 + 1.0) / (1u64 << 53) as f64
    }

    fn sample(&mut self) -> Complex<f32> {
        let radius = (-self.uniform().ln()).sqrt();
        let angle = TAU * self.uniform();
        Complex::new((radius * angle.cos()) as f32, (radius * angle.sin()) as f32)
    }

    fn block(&mut self, len: usize, amplitude: f32) -> Vec<Complex<f32>> {
        (0..len).map(|_| self.sample() * amplitude).collect()
    }
}

fn frequency(bin: usize, len: usize) -> f64 {
    if bin < len.div_ceil(2) {
        bin as f64 / len as f64
    } else {
        (bin as f64 - len as f64) / len as f64
    }
}

fn shaped(input: &[Complex<f32>], response: impl Fn(f64) -> Complex<f64>) -> Vec<Complex<f32>> {
    let len = input.len();
    let mut fft = FftPair::new(len);
    let mut buffer = input.to_vec();
    fft.forward(&mut buffer);
    for (bin, value) in buffer.iter_mut().enumerate() {
        let gain = response(frequency(bin, len));
        *value *= Complex::new(gain.re as f32, gain.im as f32);
    }
    fft.inverse_scaled(&mut buffer);
    buffer
}

#[derive(Clone, Copy, Debug)]
struct Truth {
    delay: f64,
    phase_deg: f64,
    gain_db: f64,
}

const fn truth(delay: f64, phase_deg: f64, gain_db: f64) -> Truth {
    Truth {
        delay,
        phase_deg,
        gain_db,
    }
}

fn hardware(truth: Truth) -> impl Fn(f64) -> Complex<f64> {
    move |nu| {
        if nu.abs() <= BAND {
            Complex::from_polar(
                10f64.powf(truth.gain_db / 20.0),
                truth.phase_deg.to_radians() - TAU * nu * truth.delay,
            )
        } else {
            Complex::new(0.0, 0.0)
        }
    }
}

fn scene(len: usize, lanes: &[Truth], seed: u64) -> Vec<Vec<Complex<f32>>> {
    let mut noise = Gaussian::new(seed);
    let source = noise.block(len, 0.1);
    lanes
        .iter()
        .map(|lane| {
            let mut out = shaped(&source, hardware(*lane));
            for sample in &mut out {
                *sample += noise.sample() * 0.000_1;
            }
            out
        })
        .collect()
}

fn tone(len: usize, hz: f64, amplitude: f32, phase: f64) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| {
            let turns = (hz / RATE * n as f64).fract();
            Complex::from_polar(amplitude, (TAU * turns + phase) as f32)
        })
        .collect()
}

fn add(into: &mut [Complex<f32>], from: &[Complex<f32>]) {
    for (sample, extra) in into.iter_mut().zip(from) {
        *sample += extra;
    }
}

fn job(
    id: u32,
    kind: CaptureKind,
    source: ArrayCalSource,
    lanes: Vec<Vec<Complex<f32>>>,
    decimation: usize,
) -> CaptureJob {
    let count = lanes.len();
    let len = lanes.first().map_or(0, Vec::len);
    let mut buffers = CaptureBuffers::new(count, 0);
    buffers.lanes = lanes;
    buffers.sample_rate = RATE;
    buffers.decimation = decimation;
    buffers.centers_hz = [CENTER; MAX_LANES];
    CaptureJob {
        request: CaptureRequest {
            id,
            kind,
            start: CaptureStart::Now,
            len,
            decimation,
            source,
            equaliser: true,
        },
        buffers: Box::new(buffers),
    }
}

fn run(worker: &mut Worker, job: &mut CaptureJob) -> Handled {
    let lanes = job.buffers.lanes.len();
    worker.handle(job, &mut || Box::new(CorrectionSet::identity(lanes)))
}

fn outcome(handled: &Handled) -> Result<SolveSummary, SolveFailure> {
    handled
        .solution
        .as_ref()
        .expect("a solution for the aggregator")
        .outcome
}

fn assert_close(label: &str, measured: f64, expected: f64, tolerance: f64) {
    assert!(
        (measured - expected).abs() <= tolerance,
        "{label}: measured {measured}, expected {expected}"
    );
}

fn residual_db(
    reference: &[Complex<f32>],
    lane: &[Complex<f32>],
    spectra: (&[Complex<f32>], &[Complex<f32>]),
) -> f64 {
    let mut convolver = FastConvolver::new(CORR_FFT, CORR_TAPS);
    let mut first = Vec::new();
    convolver
        .set_response(spectra.0)
        .expect("a reference response");
    convolver.push(reference, &mut first);
    convolver.reset();
    let mut second = Vec::new();
    convolver.set_response(spectra.1).expect("a lane response");
    convolver.push(lane, &mut second);
    let end = first.len().min(second.len()) - CORR_TAPS;
    let settled = CORR_TAPS..end;
    let error: f64 = first[settled.clone()]
        .iter()
        .zip(&second[settled.clone()])
        .map(|(a, b)| f64::from((a - b).norm_sqr()))
        .sum();
    let power: f64 = first[settled].iter().map(|a| f64::from(a.norm_sqr())).sum();
    10.0 * (error / power).log10()
}

#[test]
fn a_noise_solve_recovers_delay_phase_and_gain() {
    let truths = [
        truth(0.0, 0.0, 0.0),
        truth(37.37, 73.0, -2.5),
        truth(-12.2, -40.0, 1.0),
        truth(0.49, 170.0, 0.0),
    ];
    let lanes = scene(SOLVE_CAPTURE, &truths, 7);
    let mut worker = Worker::new(4, RATE);
    let mut capture = job(
        1,
        CaptureKind::Solve,
        ArrayCalSource::Noise,
        lanes.clone(),
        1,
    );
    let handled = run(&mut worker, &mut capture);
    let summary = outcome(&handled).expect("a solved capture");
    let solution = handled.solution.as_ref().expect("a solution");
    assert_eq!(
        solution.offsets.map(|offsets| offsets[..4].to_vec()),
        Some(vec![0, 37, -12, 0])
    );
    for (lane, truth) in truths.iter().enumerate().skip(1) {
        assert_close("delay", f64::from(summary.delay[lane]), truth.delay, 0.01);
        assert_close(
            "phase",
            f64::from(summary.phase_deg[lane]),
            truth.phase_deg,
            0.5,
        );
        assert_close(
            "gain",
            f64::from(summary.gain_db[lane]),
            truth.gain_db,
            0.05,
        );
    }
    assert!(summary.phase_ready && summary.gain_ready);
    assert_eq!(solution.quality.source, Some(CalSourceKind::Noise));
    assert!(solution.quality.phase_sigma_deg < 1.0);
    let set = solution.correction.as_ref().expect("a correction");
    let shifted_lane = &lanes[1][37..];
    let residual = residual_db(&lanes[0], shifted_lane, (&set.spectra[0], &set.spectra[1]));
    assert!(residual < -40.0, "{residual} dB");
    let Some(WorkerReport::Solved(detail)) = &handled.report else {
        panic!("a solve detail");
    };
    assert_close("detail delay", detail.delays[1], 37.37, 0.01);
    assert_eq!(detail.equalisers.len(), 4);
}

#[test]
fn a_fine_search_outside_the_margin_fails() {
    let truths = [truth(0.0, 0.0, 0.0), truth(3_000.0, 0.0, 0.0)];
    let lanes = scene(SOLVE_CAPTURE, &truths, 8);
    let mut worker = Worker::new(2, RATE);
    let mut capture = job(2, CaptureKind::Solve, ArrayCalSource::Noise, lanes, 1);
    assert_eq!(
        outcome(&run(&mut worker, &mut capture)),
        Err(SolveFailure::NoPeak { lane: 1 })
    );
}

fn clip_at_sigmas(lane: &mut [Complex<f32>], sigmas: f32) {
    let power = lane.iter().map(Complex::norm_sqr).sum::<f32>() / lane.len() as f32;
    let limit = sigmas * (power / 2.0).sqrt();
    for sample in lane.iter_mut() {
        *sample = Complex::new(
            sample.re.clamp(-limit, limit) / limit,
            sample.im.clamp(-limit, limit) / limit,
        );
    }
}

#[test]
fn a_noise_capture_clipped_past_the_limit_is_refused() {
    let truths = [truth(0.0, 0.0, 0.0); 3];
    let mut lanes = scene(SOLVE_CAPTURE, &truths, 9);
    clip_at_sigmas(&mut lanes[2], 0.5);
    let mut worker = Worker::new(3, RATE);
    let mut capture = job(3, CaptureKind::Solve, ArrayCalSource::Noise, lanes, 1);
    assert_eq!(
        outcome(&run(&mut worker, &mut capture)),
        Err(SolveFailure::Clipped { lane: 2 })
    );
}

#[test]
fn a_lightly_clipped_noise_capture_solves_with_a_wider_sigma() {
    let truths = [
        truth(0.0, 0.0, 0.0),
        truth(1.3, 40.0, 0.0),
        truth(0.0, -25.0, 0.0),
    ];
    let mut lanes = scene(SOLVE_CAPTURE, &truths, 10);
    clip_at_sigmas(&mut lanes[2], 2.0);
    let mut worker = Worker::new(3, RATE);
    let mut capture = job(3, CaptureKind::Solve, ArrayCalSource::Noise, lanes, 1);
    let handled = run(&mut worker, &mut capture);
    let summary = outcome(&handled).expect("a clipped capture still solves");
    assert_close("phase", f64::from(summary.phase_deg[1]), 40.0, 0.5);
    assert_close("clipped phase", f64::from(summary.phase_deg[2]), -25.0, 1.0);
    let solution = handled.solution.as_ref().expect("a solution");
    assert!(
        f64::from(solution.quality.phase_sigma_deg) >= CLIPPED_PHASE_SIGMA_DEG,
        "{}",
        solution.quality.phase_sigma_deg
    );
}

fn coarse_lanes(lags: &[usize], seed: u64) -> Vec<Vec<Complex<f32>>> {
    let reach = 4_000;
    let mut noise = Gaussian::new(seed);
    let common = noise.block(COARSE_CAPTURE + 2 * reach, 1.0);
    lags.iter()
        .map(|lag| common[*lag..*lag + COARSE_CAPTURE].to_vec())
        .collect()
}

#[test]
fn coarse_moves_lagging_lanes_onto_the_reference() {
    let factor = 64;
    let lanes = coarse_lanes(&[1_000, 300, 1_250], 11);
    let mut capture = job(4, CaptureKind::Coarse, ArrayCalSource::Noise, lanes, factor);
    capture.buffers.offsets[1] = 5;
    capture.buffers.offsets[2] = -3;
    let mut worker = Worker::new(3, RATE);
    let handled = run(&mut worker, &mut capture);
    let solution = handled.solution.as_ref().expect("a solution");
    assert!(solution.correction.is_none());
    assert_eq!(
        solution.offsets.map(|offsets| offsets[..3].to_vec()),
        Some(vec![0, 5 + 700 * 64, -3 - 250 * 64])
    );
    let summary = solution.outcome.expect("a coarse lock");
    assert!(!summary.phase_ready);
    let Some(WorkerReport::Solved(detail)) = &handled.report else {
        panic!("a coarse detail");
    };
    assert_eq!(detail.id, 4);
    assert_eq!(detail.delays[1], (5 + 700 * 64) as f64);
    assert_eq!(detail.drift_ppm, None);
}

fn clocked_lanes(cfo_hz: f64, lag: usize, seed: u64) -> Vec<Vec<Complex<f32>>> {
    let rate = RATE / 64.0;
    let reach = 2_000;
    let len = COARSE_CAPTURE + reach;
    let mut noise = Gaussian::new(seed);
    let lines = [(-9_000.0, 0.02f32), (2_100.0, 0.02), (11_000.0, 0.02)];
    let common: Vec<Complex<f32>> = (0..len)
        .map(|n| {
            let tones: Complex<f32> = lines
                .iter()
                .map(|&(hz, amplitude)| {
                    Complex::from_polar(amplitude, (TAU * (hz / rate * n as f64).fract()) as f32)
                })
                .sum();
            tones + noise.sample() * 0.125
        })
        .collect();
    let reference = common[reach..reach + COARSE_CAPTURE].to_vec();
    let lane = common[reach - lag..reach - lag + COARSE_CAPTURE]
        .iter()
        .enumerate()
        .map(|(n, sample)| {
            let turns = (cfo_hz / rate * n as f64).fract();
            sample * Complex::from_polar(1.0, (TAU * turns) as f32)
        })
        .collect();
    vec![reference, lane]
}

#[test]
fn lanes_on_other_clocks_fail_as_drift() {
    let mut capture = job(
        5,
        CaptureKind::Coarse,
        ArrayCalSource::Off,
        clocked_lanes(868.0, 400, 12),
        64,
    );
    capture.buffers.devices[1] = 1;
    let mut worker = Worker::new(2, RATE);
    match outcome(&run(&mut worker, &mut capture)) {
        Err(SolveFailure::Drift { ppm }) => assert_close("ppm", f64::from(ppm), 2.0, 0.05),
        other => panic!("expected a drift, got {other:?}"),
    }
}

#[test]
fn lanes_on_a_shared_clock_keep_a_small_offset_and_lock() {
    let mut capture = job(
        6,
        CaptureKind::Coarse,
        ArrayCalSource::Off,
        clocked_lanes(5.0, 400, 13),
        64,
    );
    capture.buffers.devices[1] = 1;
    let mut worker = Worker::new(2, RATE);
    let handled = run(&mut worker, &mut capture);
    let summary = outcome(&handled).expect("a lock");
    let offsets = handled
        .solution
        .as_ref()
        .and_then(|solution| solution.offsets)
        .expect("offsets");
    assert_eq!(offsets[1], 400 * 64);
    assert_close("cfo", f64::from(summary.cfo_hz[1]), 5.0, 2.0);
    assert_close("held cfo", worker.cfo_hz[1], 5.0, 2.0);
}

#[test]
fn a_carrier_estimate_on_a_shared_clock_leaves_the_phase_unbiased() {
    let truths = [truth(0.0, 0.0, 0.0), truth(2.3, 40.0, -1.0)];
    let mut capture = job(
        14,
        CaptureKind::Solve,
        ArrayCalSource::Noise,
        scene(SOLVE_CAPTURE, &truths, 22),
        1,
    );
    capture.buffers.devices[1] = 1;
    let mut worker = Worker::new(2, RATE);
    worker.cfo_hz[1] = 1.5;
    let summary = outcome(&run(&mut worker, &mut capture)).expect("a noise solve");
    assert_close("phase", f64::from(summary.phase_deg[1]), 40.0, 0.5);
    assert_close("delay", f64::from(summary.delay[1]), 2.3, 0.01);
}

fn pilot_lanes(truths: &[Truth], broadband: f32, seed: u64) -> Vec<Vec<Complex<f32>>> {
    let len = PILOT_CAPTURE;
    let mut noise = Gaussian::new(seed);
    let common = noise.block(len, broadband);
    let pilot = tone(len, 100e3, 0.1, 0.0);
    let emitter_phases = [0.0, 1.1, -2.0, 2.7];
    truths
        .iter()
        .zip(emitter_phases)
        .map(|(truth, phase)| {
            let mut input = common.clone();
            add(&mut input, &pilot);
            add(&mut input, &tone(len, 300e3, 0.3, phase));
            let mut out = shaped(&input, hardware(*truth));
            for sample in &mut out {
                *sample += noise.sample() * 0.001;
            }
            out
        })
        .collect()
}

#[test]
fn a_pilot_solve_ignores_an_off_air_emitter() {
    let truths = [
        truth(0.0, 0.0, 0.0),
        truth(0.3, 50.0, -1.0),
        truth(-0.2, -120.0, 0.5),
        truth(0.45, 10.0, 0.0),
    ];
    let mut worker = Worker::new(4, RATE);
    let mut noise = job(
        7,
        CaptureKind::Solve,
        ArrayCalSource::Noise,
        scene(SOLVE_CAPTURE, &truths, 14),
        1,
    );
    outcome(&run(&mut worker, &mut noise)).expect("a noise solve");
    let source = ArrayCalSource::Pilot {
        offset_hz: 100e3,
        bandwidth_hz: 10e3,
    };
    let mut pilot = job(
        8,
        CaptureKind::Solve,
        source,
        pilot_lanes(&truths, 0.01, 15),
        1,
    );
    let handled = run(&mut worker, &mut pilot);
    let summary = outcome(&handled).expect("a pilot solve");
    for (lane, truth) in truths.iter().enumerate().skip(1) {
        assert_close(
            "phase",
            f64::from(summary.phase_deg[lane]),
            truth.phase_deg,
            1.0,
        );
        assert_close("gain", f64::from(summary.gain_db[lane]), truth.gain_db, 0.1);
    }
    let quality = handled.solution.as_ref().expect("a solution").quality;
    assert_eq!(quality.source, Some(CalSourceKind::Pilot));
    assert_eq!(
        quality.valid_hz,
        Some((CENTER - 0.4 * RATE, CENTER + 0.4 * RATE))
    );
}

#[test]
fn a_pilot_without_a_delay_solve_is_valid_only_near_the_pilot() {
    let truths = [
        truth(0.0, 0.0, 0.0),
        truth(0.0, 33.0, 0.0),
        truth(0.0, -75.0, 0.0),
        truth(0.0, 140.0, 0.0),
    ];
    let source = ArrayCalSource::Pilot {
        offset_hz: 100e3,
        bandwidth_hz: 10e3,
    };
    let mut worker = Worker::new(4, RATE);
    let mut pilot = job(
        9,
        CaptureKind::Solve,
        source,
        pilot_lanes(&truths, 0.0, 16),
        1,
    );
    let handled = run(&mut worker, &mut pilot);
    let summary = outcome(&handled).expect("a pilot solve");
    for (lane, truth) in truths.iter().enumerate().skip(1) {
        assert_close(
            "phase",
            f64::from(summary.phase_deg[lane]),
            truth.phase_deg,
            1.0,
        );
    }
    let quality = handled.solution.as_ref().expect("a solution").quality;
    assert_eq!(
        quality.valid_hz,
        Some((CENTER + 100e3 - 50e3, CENTER + 100e3 + 50e3))
    );
}

fn uca(lanes: usize, radius: f64) -> Vec<Vec3> {
    (0..lanes)
        .map(|lane| {
            let angle = TAU * lane as f64 / lanes as f64;
            Vec3::new(radius * angle.sin(), radius * angle.cos(), 0.0)
        })
        .collect()
}

#[test]
fn an_emitter_solve_divides_out_the_steering() {
    let truths = [
        truth(0.0, 0.0, 0.0),
        truth(0.0, 60.0, -1.0),
        truth(0.0, -100.0, 0.0),
        truth(0.0, 15.0, 1.0),
    ];
    let offset_hz = 50e3;
    let positions = uca(4, 0.3);
    let mut steering = [Complex::new(1.0, 0.0); MAX_LANES];
    steer(
        &positions,
        CENTER + offset_hz,
        Direction::horizon(37.0 - 10.0),
        &mut steering[..4],
    );
    let len = PILOT_CAPTURE;
    let mut noise = Gaussian::new(17);
    let common = noise.block(len, 0.01);
    let emitter = tone(len, offset_hz, 0.2, 0.0);
    let lanes: Vec<Vec<Complex<f32>>> = truths
        .iter()
        .zip(&steering)
        .map(|(truth, arrival)| {
            let mut input = common.clone();
            let arriving: Vec<Complex<f32>> = emitter.iter().map(|value| value * arrival).collect();
            add(&mut input, &arriving);
            shaped(&input, hardware(*truth))
        })
        .collect();
    let source = ArrayCalSource::Emitter {
        offset_hz,
        bandwidth_hz: 5e3,
        bearing_deg: 37.0,
    };
    let mut worker = Worker::new(4, RATE);
    let mut spare = || Box::new(CorrectionSet::identity(4));
    let mut capture = job(10, CaptureKind::Solve, source, lanes.clone(), 1);
    assert_eq!(
        outcome(&run(&mut worker, &mut capture)),
        Err(SolveFailure::Refused)
    );
    assert!(
        worker
            .order(WorkerOrder::Steer { id: 11, steering }, &mut spare)
            .is_none()
    );
    let mut capture = job(11, CaptureKind::Solve, source, lanes, 1);
    let handled = run(&mut worker, &mut capture);
    let summary = outcome(&handled).expect("an emitter solve");
    for (lane, truth) in truths.iter().enumerate().skip(1) {
        assert_close(
            "phase",
            f64::from(summary.phase_deg[lane]),
            truth.phase_deg,
            1.0,
        );
        assert_close("gain", f64::from(summary.gain_db[lane]), truth.gain_db, 0.1);
    }
}

#[test]
fn a_live_check_reports_delays_without_a_solution() {
    let truths = [
        truth(0.0, 0.0, 0.0),
        truth(3.3, 20.0, 0.0),
        truth(-1.1, 0.0, 0.0),
    ];
    let mut capture = job(
        12,
        CaptureKind::Check,
        ArrayCalSource::Off,
        scene(SOLVE_CAPTURE, &truths, 18),
        1,
    );
    capture.buffers.offsets[1] = 10;
    let mut worker = Worker::new(3, RATE);
    let handled = run(&mut worker, &mut capture);
    assert!(handled.solution.is_none());
    let Some(WorkerReport::Checked(check)) = handled.report else {
        panic!("a check report");
    };
    assert_eq!(check.id, 12);
    assert_close("lane 1", check.delays[1].expect("lane 1"), 13.3, 0.05);
    assert_close("lane 2", check.delays[2].expect("lane 2"), -1.1, 0.05);
}

#[test]
fn a_time_only_solve_keeps_phase_unready() {
    let truths = [truth(0.0, 0.0, 0.0), truth(5.25, 90.0, -3.0)];
    let mut capture = job(
        13,
        CaptureKind::Solve,
        ArrayCalSource::Off,
        scene(SOLVE_CAPTURE, &truths, 19),
        1,
    );
    let mut worker = Worker::new(2, RATE);
    let handled = run(&mut worker, &mut capture);
    let summary = outcome(&handled).expect("a time solve");
    assert!(!summary.phase_ready && !summary.gain_ready);
    assert_close("delay", f64::from(summary.delay[1]), 5.25, 0.02);
    assert_eq!(summary.phase_deg[1], 0.0);
    let quality = handled.solution.as_ref().expect("a solution").quality;
    assert_eq!(quality.source, None);
}

fn stored(delay_samples: f64, phase_deg: f64, gain_db: f64) -> LaneSolution {
    LaneSolution {
        delay_samples,
        phase_deg,
        gain_db,
        coherence: 0.99,
        equaliser: Vec::new(),
    }
}

fn record() -> ArrayCalRecord {
    ArrayCalRecord {
        lanes: (0..3)
            .map(|stream| LaneKey {
                device: "kraken:1000".to_owned(),
                stream,
            })
            .collect(),
        center_hz: CENTER,
        sample_rate: RATE,
        gain_db: Some(30.0),
        source: CalSourceKind::Noise,
        keeps_phase: true,
        solved_at: "2026-09-29T10:00:00Z".to_owned(),
        solution: vec![
            stored(0.0, 0.0, 0.0),
            stored(12.25, 30.0, -1.5),
            stored(-3.4, -60.0, 2.0),
        ],
    }
}

fn warm_order(phase: bool) -> WorkerOrder {
    WorkerOrder::Warm(Box::new(WarmOrder {
        id: 20,
        record: record(),
        usage: WarmUse {
            gain: true,
            equaliser: false,
            delay_prior: true,
            phase,
        },
        offsets: true,
    }))
}

#[test]
fn a_warm_order_installs_gain_phase_and_the_delay_prior() {
    let mut worker = Worker::new(3, RATE);
    let mut spare = || Box::new(CorrectionSet::identity(3));
    let handled = worker
        .order(warm_order(true), &mut spare)
        .expect("a warm solution");
    let solution = handled.solution.expect("a solution");
    assert_eq!(solution.id, 20);
    assert_eq!(
        solution.offsets.map(|offsets| offsets[..3].to_vec()),
        Some(vec![0, 12, -3])
    );
    let summary = solution.outcome.expect("warm");
    assert!(summary.phase_ready && summary.gain_ready);
    assert_eq!(solution.quality.source, Some(CalSourceKind::Noise));
    let set = solution.correction.expect("a correction");
    let expected = Complex::from_polar(10f32.powf(1.5 / 20.0), -30f32.to_radians());
    assert!(
        (set.spectra[1][0] - expected).norm() < 1e-3,
        "{:?}",
        set.spectra[1][0]
    );
    let handled = worker
        .order(warm_order(false), &mut spare)
        .expect("a warm solution");
    let solution = handled.solution.expect("a solution");
    assert!(!solution.outcome.expect("warm").phase_ready);
    let set = solution.correction.expect("a correction");
    assert!((set.spectra[1][0] - Complex::new(10f32.powf(1.5 / 20.0), 0.0)).norm() < 1e-3);
}

#[test]
fn pilot_rates_divide_the_array_rate() {
    assert_close("wide", pilot_rate(RATE, 10e3), 40e3, 1e-9);
    assert_close("narrow", pilot_rate(RATE, 100.0), 4e3, 1e-9);
    assert_close("odd", pilot_rate(1e6, 30e3), 1e6 / 8.0, 1e-9);
}

fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(Instant::now() < deadline, "the worker never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_worker_thread_answers_jobs_and_returns_buffers() {
    let (mut jobs_tx, jobs) = RingBuffer::new(2);
    let (solutions, mut solutions_rx) = RingBuffer::new(4);
    let (buffers, mut buffers_rx) = RingBuffer::new(2);
    let (mut sets_tx, sets) = RingBuffer::new(2);
    let _ = sets_tx.push(Box::new(CorrectionSet::identity(2)));
    let (link, orders, reports) = link();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = spawn_worker(
        "sdrmm-array-sync-test".to_owned(),
        WorkerIo {
            lanes: 2,
            sample_rate: RATE,
            jobs,
            solutions,
            buffers,
            sets,
            stop: stop.clone(),
            orders,
            reports,
        },
    )
    .expect("a worker");
    let truths = [truth(0.0, 0.0, 0.0), truth(2.4, 45.0, 0.0)];
    let lanes = scene(SOLVE_CAPTURE, &truths, 21);
    let _ = jobs_tx.push(job(
        30,
        CaptureKind::Check,
        ArrayCalSource::Off,
        lanes.clone(),
        1,
    ));
    handle.thread().unpark();
    let report = wait_for(|| link.reports.try_recv().ok());
    assert!(matches!(report, WorkerReport::Checked(check) if check.id == 30));
    let _ = jobs_tx.push(job(31, CaptureKind::Solve, ArrayCalSource::Noise, lanes, 1));
    handle.thread().unpark();
    let solution = wait_for(|| solutions_rx.pop().ok());
    assert_eq!(solution.id, 31);
    assert!(solution.outcome.is_ok());
    assert!(matches!(
        link.reports.try_recv(),
        Ok(WorkerReport::Solved(detail)) if detail.id == 31
    ));
    let returned = std::iter::from_fn(|| buffers_rx.pop().ok()).count();
    assert_eq!(returned, 2);
    stop.store(true, Ordering::Release);
    handle.thread().unpark();
    handle.join().expect("the worker stops");
}
