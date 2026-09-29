use std::{
    f64::consts::TAU,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError};
use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_dsp::{
    Ddc,
    array_sync::{
        BinError, BinSolution, BinSolver, COARSE_FRAME, COARSE_LAGS, CoarseError, CoarseSearch,
        FIT_BAND, POWER_ITERATIONS, design_correction, dominant,
    },
    xcorr::XCorr,
};
use sdrmm_wire::{ArrayCalRecord, ArrayCalSource};

use super::{
    CaptureBuffers, CaptureJob, CorrectionSet, Solution,
    capture::{CalQuality, CaptureKind, SolveFailure, SolveSummary},
    correct::{CORR_BETA, CORR_FFT, CORR_TAPS},
    track::DRIFT_FAIL_PPM,
    warm::{self, WarmUse},
};
use crate::EngineError;

pub(crate) const FINE_MARGIN: usize = 2_048;
pub(crate) const FINE_FRAME: usize = 16_384;
pub(crate) const FINE_SPAN: usize = FINE_FRAME + 2 * FINE_MARGIN;
pub(crate) const SOLVE_LEN: usize = 65_536;
pub(crate) const SOLVE_CAPTURE: usize = SOLVE_LEN + 2 * FINE_MARGIN;
pub(crate) const PILOT_CAPTURE: usize = 131_072;
pub(crate) const COARSE_CAPTURE: usize = COARSE_FRAME + 2 * COARSE_LAGS;
pub(crate) const BIN_FFT: usize = 1_024;
pub(crate) const PILOT_PURITY: f32 = 0.7;
pub(crate) const PILOT_MIN_RATE_HZ: f64 = 4_000.0;
pub(crate) const PILOT_SPAN_WIDTHS: f64 = 5.0;
const PEAK_TO_FLOOR_DB: f32 = 20.0;
const PILOT_SETTLE_FRACTION: usize = 8;
const CLIP_LEVEL: f32 = 0.999;
const WORKER_PARK: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Thresholds {
    xcorr: f32,
    purity: f32,
    lane: f32,
}

const NOISE: Thresholds = Thresholds {
    xcorr: 0.5,
    purity: 0.8,
    lane: 0.9,
};

const LIVE: Thresholds = Thresholds {
    xcorr: 0.3,
    purity: 0.5,
    lane: 0.6,
};

pub(crate) struct WarmOrder {
    pub(crate) id: u32,
    pub(crate) record: ArrayCalRecord,
    pub(crate) usage: WarmUse,
    pub(crate) offsets: bool,
}

pub(crate) enum WorkerOrder {
    Warm(Box<WarmOrder>),
    Steer {
        id: u32,
        steering: [Complex<f32>; MAX_LANES],
    },
}

impl WorkerOrder {
    const fn id(&self) -> u32 {
        match self {
            Self::Warm(order) => order.id,
            Self::Steer { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SolveDetail {
    pub(crate) id: u32,
    pub(crate) delays: [f64; MAX_LANES],
    pub(crate) equalisers: Vec<Vec<Complex<f32>>>,
    pub(crate) drift_ppm: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CheckDetail {
    pub(crate) id: u32,
    pub(crate) delays: [Option<f64>; MAX_LANES],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WorkerReport {
    Solved(Box<SolveDetail>),
    Checked(Box<CheckDetail>),
}

pub(crate) struct SyncLink {
    pub(crate) orders: mpsc::Sender<WorkerOrder>,
    pub(crate) reports: mpsc::Receiver<WorkerReport>,
}

pub(crate) fn link() -> (
    SyncLink,
    mpsc::Receiver<WorkerOrder>,
    mpsc::Sender<WorkerReport>,
) {
    let (orders_tx, orders_rx) = mpsc::channel();
    let (reports_tx, reports_rx) = mpsc::channel();
    (
        SyncLink {
            orders: orders_tx,
            reports: reports_rx,
        },
        orders_rx,
        reports_tx,
    )
}

pub(crate) struct WorkerIo {
    pub(crate) lanes: usize,
    pub(crate) sample_rate: f64,
    pub(crate) jobs: Consumer<CaptureJob>,
    pub(crate) solutions: Producer<Box<Solution>>,
    pub(crate) buffers: Producer<Box<CaptureBuffers>>,
    pub(crate) sets: Consumer<Box<CorrectionSet>>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) orders: mpsc::Receiver<WorkerOrder>,
    pub(crate) reports: mpsc::Sender<WorkerReport>,
}

pub(crate) fn spawn_worker(name: String, io: WorkerIo) -> Result<JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || serve(io))
        .map_err(|error| EngineError::Processor(format!("start array sync worker: {error}")))
}

fn serve(mut io: WorkerIo) {
    let mut worker = Worker::new(io.lanes, io.sample_rate);
    let mut held: Option<Box<Solution>> = None;
    while !io.stop.load(Ordering::Acquire) {
        let mut worked = false;
        if let Some(solution) = held.take() {
            held = deliver(&mut io.solutions, solution);
            worked = held.is_none();
        }
        if held.is_none()
            && let Some(handled) = next(&mut worker, &mut io)
        {
            worked = true;
            if let Some(report) = handled.report {
                let _ = io.reports.send(report);
            }
            held = handled
                .solution
                .and_then(|solution| deliver(&mut io.solutions, solution));
        }
        if !worked {
            std::thread::park_timeout(WORKER_PARK);
        }
    }
}

fn next(worker: &mut Worker, io: &mut WorkerIo) -> Option<Handled> {
    let lanes = io.lanes;
    let sets = &mut io.sets;
    let mut spare = || spare_set(sets, lanes);
    if let Ok(order) = io.orders.try_recv() {
        let id = order.id();
        return catch_unwind(AssertUnwindSafe(|| worker.order(order, &mut spare))).unwrap_or_else(
            |_| {
                tracing::error!(id, "the array sync worker failed on an order");
                Some(Handled::failed(id, SolveFailure::Refused))
            },
        );
    }
    let mut job = io.jobs.pop().ok()?;
    let id = job.request.id;
    let handled = catch_unwind(AssertUnwindSafe(|| worker.handle(&mut job, &mut spare)))
        .unwrap_or_else(|_| {
            tracing::error!(id, "the array sync worker failed on a capture");
            Handled::failed(id, SolveFailure::Refused)
        });
    if let Err(PushError::Full(buffers)) = io.buffers.push(job.buffers) {
        tracing::error!("the array capture pool is full, a capture buffer was dropped");
        drop(buffers);
    }
    Some(handled)
}

fn spare_set(sets: &mut Consumer<Box<CorrectionSet>>, lanes: usize) -> Box<CorrectionSet> {
    sets.pop()
        .unwrap_or_else(|_| Box::new(CorrectionSet::identity(lanes)))
}

fn deliver(
    solutions: &mut Producer<Box<Solution>>,
    solution: Box<Solution>,
) -> Option<Box<Solution>> {
    match solutions.push(solution) {
        Ok(()) => None,
        Err(PushError::Full(solution)) => Some(solution),
    }
}

pub(crate) struct Handled {
    pub(crate) solution: Option<Box<Solution>>,
    pub(crate) report: Option<WorkerReport>,
}

impl Handled {
    fn failed(id: u32, failure: SolveFailure) -> Self {
        Self {
            solution: Some(Box::new(Solution {
                id,
                offsets: None,
                correction: None,
                outcome: Err(failure),
                quality: CalQuality::default(),
            })),
            report: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Fine {
    delay: f64,
    coherence: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Solved {
    lanes: usize,
    offsets: [i64; MAX_LANES],
    frac: [f64; MAX_LANES],
    phase_rad: [f64; MAX_LANES],
    gain: [f64; MAX_LANES],
    coherence: [f32; MAX_LANES],
    equalisers: Vec<Vec<Complex<f32>>>,
    purity: f32,
    phase_ready: bool,
    apply_eq: bool,
    quality: CalQuality,
}

#[derive(Clone, Debug, PartialEq)]
struct Narrow {
    response: [Complex<f64>; MAX_LANES],
    coherence: [f32; MAX_LANES],
    purity: f32,
    snapshots: usize,
}

pub(crate) struct Worker {
    lanes: usize,
    sample_rate: f64,
    coarse: Option<CoarseSearch>,
    fine: XCorr,
    bins: BinSolver,
    cfo_hz: [f64; MAX_LANES],
    noise_frac: [Option<f64>; MAX_LANES],
    live_frac: [Option<f64>; MAX_LANES],
    magnitudes: Vec<f32>,
    steering: Option<(u32, [Complex<f32>; MAX_LANES])>,
    narrow: Vec<Vec<Complex<f32>>>,
}

impl Worker {
    pub(crate) fn new(lanes: usize, sample_rate: f64) -> Self {
        let lanes = lanes.min(MAX_LANES);
        Self {
            lanes,
            sample_rate,
            coarse: None,
            fine: XCorr::new(FINE_SPAN),
            bins: BinSolver::new(lanes, BIN_FFT),
            cfo_hz: [0.0; MAX_LANES],
            noise_frac: [None; MAX_LANES],
            live_frac: [None; MAX_LANES],
            magnitudes: Vec::with_capacity(2 * FINE_SPAN),
            steering: None,
            narrow: vec![Vec::new(); lanes],
        }
    }

    pub(crate) fn order(
        &mut self,
        order: WorkerOrder,
        spare: &mut dyn FnMut() -> Box<CorrectionSet>,
    ) -> Option<Handled> {
        match order {
            WorkerOrder::Steer { id, steering } => {
                self.steering = Some((id, steering));
                None
            }
            WorkerOrder::Warm(order) => Some(self.warm(&order, spare())),
        }
    }

    pub(crate) fn handle(
        &mut self,
        job: &mut CaptureJob,
        spare: &mut dyn FnMut() -> Box<CorrectionSet>,
    ) -> Handled {
        let id = job.request.id;
        if job.buffers.lanes.len() != self.lanes {
            return Handled::failed(id, SolveFailure::Short);
        }
        match job.request.kind {
            CaptureKind::Coarse => self.coarse_job(job),
            CaptureKind::Solve => match self.solve_job(job) {
                Ok(solved) => self.publish(id, &solved, spare),
                Err(failure) => Handled::failed(id, failure),
            },
            CaptureKind::Check => self.check_job(job),
        }
    }

    fn rate(&self, buffers: &CaptureBuffers) -> f64 {
        let rate = buffers.sample_rate / buffers.decimation.max(1) as f64;
        if rate.is_finite() && rate > 0.0 {
            rate
        } else {
            self.sample_rate
        }
    }

    fn coarse_job(&mut self, job: &CaptureJob) -> Handled {
        let id = job.request.id;
        let buffers = &job.buffers;
        let rate = self.rate(buffers);
        let factor = buffers.decimation.max(1) as i64;
        let search = self
            .coarse
            .get_or_insert_with(|| CoarseSearch::new(COARSE_FRAME, COARSE_LAGS));
        let mut offsets = buffers.offsets;
        let mut cfo_hz = [0.0; MAX_LANES];
        let mut drift_ppm: Option<f64> = None;
        let reference = &buffers.lanes[0];
        for (lane, samples) in buffers.lanes.iter().enumerate().skip(1) {
            let shared = same_device(buffers, 0, lane);
            let found = if shared {
                search.lag(reference, samples)
            } else {
                search.lag_across_clocks(reference, samples, rate)
            };
            let found = match found {
                Ok(found) => found,
                Err(error) => return Handled::failed(id, coarse_failure(error, lane)),
            };
            offsets[lane] = offsets[lane].saturating_add(found.lag.saturating_mul(factor));
            cfo_hz[lane] = found.cfo_hz;
            let center = buffers.centers_hz[lane];
            if !shared && center.is_finite() && center.abs() > 0.0 {
                let ppm = found.cfo_hz / center * 1e6;
                if drift_ppm.is_none_or(|held| ppm.abs() > held.abs()) {
                    drift_ppm = Some(ppm);
                }
            }
        }
        self.cfo_hz = cfo_hz;
        if let Some(ppm) = drift_ppm.filter(|ppm| ppm.abs() > DRIFT_FAIL_PPM) {
            return Handled::failed(id, SolveFailure::Drift { ppm: ppm as f32 });
        }
        let lanes = buffers.lanes.len();
        let mut summary = SolveSummary {
            lanes: lanes as u8,
            ..SolveSummary::default()
        };
        let mut delays = [0.0; MAX_LANES];
        for lane in 0..lanes {
            delays[lane] = offsets[lane] as f64;
            summary.delay[lane] = delays[lane] as f32;
            summary.cfo_hz[lane] = cfo_hz[lane] as f32;
        }
        Handled {
            solution: Some(Box::new(Solution {
                id,
                offsets: Some(offsets),
                correction: None,
                outcome: Ok(summary),
                quality: CalQuality::default(),
            })),
            report: Some(WorkerReport::Solved(Box::new(SolveDetail {
                id,
                delays,
                equalisers: Vec::new(),
                drift_ppm,
            }))),
        }
    }

    fn derotate(&self, buffers: &mut CaptureBuffers) {
        let rate = self.rate(buffers);
        let devices = buffers.devices;
        for (lane, samples) in buffers.lanes.iter_mut().enumerate().skip(1) {
            let cfo = self.cfo_hz[lane];
            if devices[lane] != devices[0] && cfo != 0.0 && cfo.is_finite() {
                rotate(samples, -cfo / rate);
            }
        }
    }

    fn fine(
        &mut self,
        lanes: &[Vec<Complex<f32>>],
        thresholds: Thresholds,
    ) -> [Result<Fine, SolveFailure>; MAX_LANES] {
        let mut found = [Err(SolveFailure::Short); MAX_LANES];
        let Some(reference) = lanes
            .first()
            .and_then(|lane| lane.get(FINE_MARGIN..FINE_MARGIN + FINE_FRAME))
        else {
            return found;
        };
        found[0] = Ok(Fine {
            delay: 0.0,
            coherence: 1.0,
        });
        let energy: f32 = reference.iter().map(Complex::norm_sqr).sum();
        for (lane, samples) in lanes.iter().enumerate().skip(1) {
            let Some(span) = samples.get(..FINE_SPAN) else {
                continue;
            };
            self.fine.correlate(reference, span, &mut self.magnitudes);
            found[lane] = judge(&self.magnitudes, span, energy)
                .filter(|fine| {
                    fine.coherence >= thresholds.xcorr && fine.peak_to_floor_db >= PEAK_TO_FLOOR_DB
                })
                .map(|fine| Fine {
                    delay: fine.delay,
                    coherence: fine.coherence,
                })
                .ok_or(SolveFailure::NoPeak { lane: lane as u8 });
        }
        found
    }

    fn solve_job(&mut self, job: &mut CaptureJob) -> Result<Solved, SolveFailure> {
        let source = job.request.source;
        let noise = matches!(source, ArrayCalSource::Noise);
        let thresholds = if noise { NOISE } else { LIVE };
        if noise && let Some(lane) = clipped(&job.buffers) {
            return Err(SolveFailure::Clipped { lane });
        }
        self.derotate(&mut job.buffers);
        let fine = self.fine(&job.buffers.lanes, thresholds);
        let buffers = &job.buffers;
        let span = buffers.lanes[0].len().saturating_sub(2 * FINE_MARGIN);
        let mut solved = Solved {
            lanes: self.lanes,
            offsets: buffers.offsets,
            gain: [1.0; MAX_LANES],
            ..Solved::default()
        };
        let (shifts, known) = match source {
            ArrayCalSource::Pilot { .. } | ArrayCalSource::Emitter { .. } => {
                self.narrow_delays(&mut solved, &fine)
            }
            ArrayCalSource::Noise | ArrayCalSource::Off => (
                self.wide_delays(&mut solved, &fine, buffers, span, noise)?,
                true,
            ),
        };
        let rate = self.rate(buffers);
        let center = buffers.centers_hz[0];
        match source {
            ArrayCalSource::Noise => {
                solved.phase_ready = true;
                solved.apply_eq = job.request.equaliser;
                solved.quality = bin_quality(&solved, span, rate, center);
            }
            ArrayCalSource::Pilot {
                offset_hz,
                bandwidth_hz,
            }
            | ArrayCalSource::Emitter {
                offset_hz,
                bandwidth_hz,
                ..
            } => {
                let steering = match source {
                    ArrayCalSource::Emitter { .. } => Some(self.steering_for(job.request.id)?),
                    _ => None,
                };
                let views = shifted(&buffers.lanes, &shifts, span).ok_or(SolveFailure::Short)?;
                let narrow = self.narrowband(&views, rate, offset_hz, bandwidth_hz, steering)?;
                take_narrow(&mut solved, &narrow, offset_hz, rate);
                solved.quality =
                    narrow_quality(&narrow, known, center, rate, offset_hz, bandwidth_hz);
            }
            ArrayCalSource::Off => {
                solved.phase_rad = [0.0; MAX_LANES];
                solved.gain = [1.0; MAX_LANES];
            }
        }
        solved.quality.source = if solved.phase_ready {
            source.kind()
        } else {
            None
        };
        Ok(solved)
    }

    fn wide_delays(
        &mut self,
        solved: &mut Solved,
        fine: &[Result<Fine, SolveFailure>; MAX_LANES],
        buffers: &CaptureBuffers,
        span: usize,
        noise: bool,
    ) -> Result<[i64; MAX_LANES], SolveFailure> {
        let mut shifts = [0i64; MAX_LANES];
        for (lane, shift) in shifts.iter_mut().enumerate().take(self.lanes) {
            *shift = fine[lane]?.delay.round() as i64;
        }
        let views = shifted(&buffers.lanes, &shifts, span).ok_or(SolveFailure::Short)?;
        let thresholds = if noise { NOISE } else { LIVE };
        match self.bins.solve(&views, thresholds.purity, thresholds.lane) {
            Ok(solution) => self.take_bins(solved, &solution, noise),
            Err(error) if noise => return Err(bin_failure(error)),
            Err(_) => {
                for lane in 0..self.lanes {
                    let found = fine[lane]?;
                    solved.frac[lane] = found.delay - shifts[lane] as f64;
                    solved.coherence[lane] = found.coherence;
                }
            }
        }
        for (lane, shift) in shifts.iter_mut().enumerate().take(self.lanes) {
            let whole = solved.frac[lane].round();
            solved.frac[lane] -= whole;
            *shift += whole as i64;
            solved.offsets[lane] = solved.offsets[lane].saturating_add(*shift);
            if noise {
                self.noise_frac[lane] = Some(solved.frac[lane]);
            } else {
                self.live_frac[lane] = Some(solved.frac[lane]);
            }
        }
        Ok(shifts)
    }

    fn narrow_delays(
        &mut self,
        solved: &mut Solved,
        fine: &[Result<Fine, SolveFailure>; MAX_LANES],
    ) -> ([i64; MAX_LANES], bool) {
        let mut shifts = [0i64; MAX_LANES];
        let mut known = true;
        for lane in 0..self.lanes {
            let measured = fine[lane].ok();
            let frac = self.noise_frac[lane]
                .or_else(|| measured.map(|found| found.delay - found.delay.round()))
                .or(self.live_frac[lane]);
            known &= frac.is_some();
            let frac = frac.unwrap_or(0.0);
            shifts[lane] = measured.map_or(0, |found| (found.delay - frac).round() as i64);
            if let Some(found) = measured {
                self.live_frac[lane] = Some(found.delay - found.delay.round());
            }
            solved.frac[lane] = frac;
            solved.coherence[lane] = measured.map_or(0.0, |found| found.coherence);
            solved.offsets[lane] = solved.offsets[lane].saturating_add(shifts[lane]);
        }
        (shifts, known)
    }

    fn take_bins(&self, solved: &mut Solved, solution: &BinSolution, noise: bool) {
        for (lane, response) in solution.lanes.iter().enumerate().take(self.lanes) {
            solved.frac[lane] = f64::from(response.delay_frac);
            solved.phase_rad[lane] = f64::from(response.phase_rad);
            solved.gain[lane] = f64::from(response.gain);
            solved.coherence[lane] = response.coherence;
        }
        solved.purity = solution.purity;
        if noise {
            solved.equalisers = solution
                .lanes
                .iter()
                .map(|response| response.equaliser.clone())
                .collect();
        }
    }

    fn steering_for(&self, id: u32) -> Result<[Complex<f32>; MAX_LANES], SolveFailure> {
        match self.steering {
            Some((held, steering)) if held == id => Ok(steering),
            _ => Err(SolveFailure::Refused),
        }
    }

    fn narrowband(
        &mut self,
        views: &[&[Complex<f32>]],
        rate: f64,
        offset_hz: f64,
        bandwidth_hz: f64,
        steering: Option<[Complex<f32>; MAX_LANES]>,
    ) -> Result<Narrow, SolveFailure> {
        let output_rate = pilot_rate(rate, bandwidth_hz);
        for (view, out) in views.iter().zip(&mut self.narrow) {
            let mut ddc =
                Ddc::new(rate, output_rate, offset_hz).map_err(|_| SolveFailure::Short)?;
            ddc.process(view, out);
        }
        let order = views.len();
        let snapshots = self.narrow.iter().map(Vec::len).min().unwrap_or(0);
        let settle = snapshots / PILOT_SETTLE_FRACTION;
        if snapshots <= settle + order {
            return Err(SolveFailure::Short);
        }
        let matrix = covariance(&self.narrow[..order], settle, snapshots);
        let mut vector = vec![Complex::default(); order];
        let lambda = dominant(&matrix, order, POWER_ITERATIONS, &mut vector);
        let trace: f32 = (0..order).map(|lane| matrix[lane * order + lane].re).sum();
        let purity = if trace > 0.0 { lambda / trace } else { 0.0 };
        let mut coherence = [0.0f32; MAX_LANES];
        coherence[0] = 1.0;
        for (lane, value) in coherence.iter_mut().enumerate().take(order).skip(1) {
            let power = matrix[0].re * matrix[lane * order + lane].re;
            *value = if power > 0.0 {
                (matrix[lane].norm_sqr() / power).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        if purity.is_nan() || purity < PILOT_PURITY {
            let (lane, weakest) = coherence[..order]
                .iter()
                .enumerate()
                .skip(1)
                .min_by(|a, b| a.1.total_cmp(b.1))
                .map_or((0, 0.0), |(lane, value)| (lane, *value));
            return Err(SolveFailure::LowCoherence {
                lane: lane as u8,
                coherence: weakest,
            });
        }
        let reference = widen(vector[0]) / steered(steering, 0);
        if reference.norm().is_nan() || reference.norm() <= f64::MIN_POSITIVE {
            return Err(SolveFailure::FewBins);
        }
        let mut response = [Complex::new(0.0, 0.0); MAX_LANES];
        for (lane, value) in vector.iter().enumerate() {
            response[lane] = widen(*value) / steered(steering, lane) / reference;
        }
        Ok(Narrow {
            response,
            coherence,
            purity,
            snapshots: snapshots - settle,
        })
    }

    fn check_job(&mut self, job: &mut CaptureJob) -> Handled {
        let id = job.request.id;
        self.derotate(&mut job.buffers);
        let buffers = &job.buffers;
        let fine = self.fine(&buffers.lanes, LIVE);
        let mut shifts = [0i64; MAX_LANES];
        for (shift, found) in shifts.iter_mut().zip(&fine) {
            *shift = found.map_or(0, |found| found.delay.round() as i64);
        }
        let span = buffers.lanes[0].len().saturating_sub(2 * FINE_MARGIN);
        let refined = shifted(&buffers.lanes, &shifts, span)
            .and_then(|views| self.bins.solve(&views, LIVE.purity, LIVE.lane).ok());
        let mut delays = [None; MAX_LANES];
        for (lane, found) in fine.iter().enumerate().take(self.lanes) {
            let Ok(found) = found else {
                continue;
            };
            let frac = refined
                .as_ref()
                .and_then(|solution| solution.lanes.get(lane))
                .map_or(found.delay - shifts[lane] as f64, |response| {
                    f64::from(response.delay_frac)
                });
            delays[lane] = Some(buffers.offsets[lane] as f64 + shifts[lane] as f64 + frac);
        }
        Handled {
            solution: None,
            report: Some(WorkerReport::Checked(Box::new(CheckDetail { id, delays }))),
        }
    }

    fn publish(
        &self,
        id: u32,
        solved: &Solved,
        spare: &mut dyn FnMut() -> Box<CorrectionSet>,
    ) -> Handled {
        let mut set = spare();
        let eq = solved.apply_eq.then_some(solved.equalisers.as_slice());
        build(&mut set, solved, eq);
        let mut summary = SolveSummary {
            lanes: solved.lanes as u8,
            purity: solved.purity,
            phase_ready: solved.phase_ready,
            gain_ready: solved.phase_ready,
            ..SolveSummary::default()
        };
        let mut delays = [0.0; MAX_LANES];
        for (lane, delay) in delays.iter_mut().enumerate().take(solved.lanes) {
            *delay = solved.offsets[lane] as f64 + solved.frac[lane];
            summary.delay[lane] = *delay as f32;
            summary.phase_deg[lane] = solved.phase_rad[lane].to_degrees() as f32;
            summary.gain_db[lane] = gain_db(solved.gain[lane]) as f32;
            summary.coherence[lane] = solved.coherence[lane];
            summary.cfo_hz[lane] = self.cfo_hz[lane] as f32;
        }
        Handled {
            solution: Some(Box::new(Solution {
                id,
                offsets: Some(solved.offsets),
                correction: Some(set),
                outcome: Ok(summary),
                quality: solved.quality,
            })),
            report: Some(WorkerReport::Solved(Box::new(SolveDetail {
                id,
                delays,
                equalisers: solved.equalisers.clone(),
                drift_ppm: None,
            }))),
        }
    }

    fn warm(&mut self, order: &WarmOrder, mut set: Box<CorrectionSet>) -> Handled {
        let record = &order.record;
        if record.solution.len() != self.lanes {
            return Handled::failed(order.id, SolveFailure::Refused);
        }
        let usage = order.usage;
        let mut solved = Solved {
            lanes: self.lanes,
            phase_ready: usage.phase,
            apply_eq: usage.equaliser,
            ..Solved::default()
        };
        let mut summary = SolveSummary {
            lanes: self.lanes as u8,
            phase_ready: usage.phase,
            gain_ready: usage.gain,
            ..SolveSummary::default()
        };
        for (lane, stored) in record.solution.iter().enumerate() {
            let whole = stored.delay_samples.round();
            solved.offsets[lane] = whole as i64;
            solved.frac[lane] = if order.offsets {
                stored.delay_samples - whole
            } else {
                0.0
            };
            solved.gain[lane] = if usage.gain {
                10f64.powf(stored.gain_db / 20.0)
            } else {
                1.0
            };
            solved.phase_rad[lane] = if usage.phase {
                stored.phase_deg.to_radians()
            } else {
                0.0
            };
            if usage.equaliser {
                solved
                    .equalisers
                    .push(warm::equaliser(stored).unwrap_or_default());
            }
            summary.delay[lane] = stored.delay_samples as f32;
            summary.phase_deg[lane] = solved.phase_rad[lane].to_degrees() as f32;
            summary.gain_db[lane] = gain_db(solved.gain[lane]) as f32;
            summary.coherence[lane] = stored.coherence;
        }
        let eq = solved.apply_eq.then_some(solved.equalisers.as_slice());
        build(&mut set, &solved, eq);
        let quality = CalQuality {
            source: Some(record.source),
            ..CalQuality::default()
        };
        Handled {
            solution: Some(Box::new(Solution {
                id: order.id,
                offsets: order.offsets.then_some(solved.offsets),
                correction: Some(set),
                outcome: Ok(summary),
                quality,
            })),
            report: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Peak {
    delay: f64,
    coherence: f32,
    peak_to_floor_db: f32,
}

fn judge(magnitudes: &[f32], span: &[Complex<f32>], energy: f32) -> Option<Peak> {
    let zero = FINE_SPAN;
    let valid = magnitudes.get(zero..=zero + 2 * FINE_MARGIN)?;
    let (at, best) = valid
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    let index = zero + at;
    let peak = best * best;
    let total: f32 = magnitudes.iter().map(|value| value * value).sum();
    let floor = (total - peak) / magnitudes.len().saturating_sub(1).max(1) as f32;
    let left = magnitudes
        .get(index.wrapping_sub(1))
        .copied()
        .unwrap_or(0.0);
    let right = magnitudes.get(index + 1).copied().unwrap_or(0.0);
    let bend = left - 2.0 * best + right;
    let fraction = if bend.abs() > f32::MIN_POSITIVE {
        (0.5 * (left - right) / bend).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let window: f32 = span
        .get(at..at + FINE_FRAME)?
        .iter()
        .map(Complex::norm_sqr)
        .sum();
    let side = left.max(right);
    let scale = energy * window;
    if !(scale > f32::MIN_POSITIVE && peak > 0.0) {
        return None;
    }
    Some(Peak {
        delay: at as f64 + f64::from(fraction) - FINE_MARGIN as f64,
        coherence: ((peak + side * side) / scale).clamp(0.0, 1.0),
        peak_to_floor_db: if floor > f32::MIN_POSITIVE {
            10.0 * (peak / floor).log10()
        } else {
            f32::INFINITY
        },
    })
}

fn build(set: &mut CorrectionSet, solved: &Solved, eq: Option<&[Vec<Complex<f32>>]>) {
    set.spectra
        .resize_with(solved.lanes, || Vec::with_capacity(CORR_FFT));
    for (lane, spectrum) in set.spectra.iter_mut().enumerate() {
        let gain = solved.gain[lane];
        let weight = if lane == 0 || !(gain > 0.0 && gain.is_finite()) {
            Complex::new(1.0, 0.0)
        } else {
            let weight = Complex::from_polar(1.0 / gain, -solved.phase_rad[lane]);
            Complex::new(weight.re as f32, weight.im as f32)
        };
        let equaliser = eq
            .filter(|_| lane > 0)
            .and_then(|lanes| lanes.get(lane))
            .map(Vec::as_slice)
            .filter(|points| !points.is_empty());
        let frac = if lane == 0 { 0.0 } else { solved.frac[lane] };
        design_correction(
            CORR_FFT,
            CORR_TAPS,
            CORR_BETA,
            frac as f32,
            weight,
            equaliser,
            spectrum,
        );
    }
}

fn take_narrow(solved: &mut Solved, narrow: &Narrow, offset_hz: f64, rate: f64) {
    for lane in 0..solved.lanes {
        let response = narrow.response[lane];
        let turn = TAU * offset_hz * solved.frac[lane] / rate;
        solved.phase_rad[lane] = wrap_rad(response.arg() + turn);
        solved.gain[lane] = response.norm();
        solved.coherence[lane] = narrow.coherence[lane];
    }
    solved.phase_rad[0] = 0.0;
    solved.gain[0] = 1.0;
    solved.purity = narrow.purity;
    solved.phase_ready = true;
    solved.equalisers.clear();
}

fn bin_quality(solved: &Solved, span: usize, rate: f64, center: f64) -> CalQuality {
    let blocks = span.saturating_sub(BIN_FFT) / (BIN_FFT / 2) + 1;
    let sigma = solved.coherence[1..solved.lanes]
        .iter()
        .map(|coherence| phase_sigma(*coherence, blocks))
        .fold(0.0, f64::max);
    CalQuality {
        source: None,
        phase_sigma_deg: sigma.to_degrees() as f32,
        gain_sigma_db: gain_db(1.0 + sigma) as f32,
        valid_hz: Some((center - FIT_BAND * rate, center + FIT_BAND * rate)),
    }
}

fn narrow_quality(
    narrow: &Narrow,
    known: bool,
    center: f64,
    rate: f64,
    offset_hz: f64,
    bandwidth_hz: f64,
) -> CalQuality {
    let sigma = phase_sigma(narrow.purity, narrow.snapshots);
    let valid_hz = if known {
        (center - FIT_BAND * rate, center + FIT_BAND * rate)
    } else {
        let spread = PILOT_SPAN_WIDTHS * bandwidth_hz;
        (center + offset_hz - spread, center + offset_hz + spread)
    };
    CalQuality {
        source: None,
        phase_sigma_deg: sigma.to_degrees() as f32,
        gain_sigma_db: gain_db(1.0 + sigma) as f32,
        valid_hz: Some(valid_hz),
    }
}

fn phase_sigma(coherence: f32, snapshots: usize) -> f64 {
    let coherence = f64::from(coherence).clamp(1e-6, 1.0);
    ((1.0 - coherence) / (2.0 * coherence * snapshots.max(1) as f64)).sqrt()
}

fn gain_db(gain: f64) -> f64 {
    if gain > 0.0 && gain.is_finite() {
        20.0 * gain.log10()
    } else {
        0.0
    }
}

pub(crate) fn pilot_rate(rate: f64, bandwidth_hz: f64) -> f64 {
    let wanted = (4.0 * bandwidth_hz).max(PILOT_MIN_RATE_HZ);
    let factor = (rate / wanted).floor().max(1.0);
    rate / factor
}

fn covariance(lanes: &[Vec<Complex<f32>>], from: usize, until: usize) -> Vec<Complex<f32>> {
    let order = lanes.len();
    let mut matrix = vec![Complex::<f64>::default(); order * order];
    for at in from..until {
        for (row, cells) in lanes.iter().zip(matrix.chunks_exact_mut(order)) {
            let left = widen(row[at]);
            for (cell, column) in cells.iter_mut().zip(lanes) {
                *cell += left * widen(column[at]).conj();
            }
        }
    }
    let scale = 1.0 / (until - from).max(1) as f64;
    matrix
        .iter()
        .map(|value| Complex::new((value.re * scale) as f32, (value.im * scale) as f32))
        .collect()
}

fn shifted<'a>(
    lanes: &'a [Vec<Complex<f32>>],
    shifts: &[i64],
    span: usize,
) -> Option<Vec<&'a [Complex<f32>]>> {
    lanes
        .iter()
        .zip(shifts)
        .map(|(lane, shift)| {
            let start = usize::try_from(FINE_MARGIN as i64 + shift).ok()?;
            lane.get(start..start + span)
        })
        .collect()
}

fn steered(steering: Option<[Complex<f32>; MAX_LANES]>, lane: usize) -> Complex<f64> {
    steering.map_or(Complex::new(1.0, 0.0), |steering| widen(steering[lane]))
}

fn widen(value: Complex<f32>) -> Complex<f64> {
    Complex::new(f64::from(value.re), f64::from(value.im))
}

fn wrap_rad(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
}

fn rotate(samples: &mut [Complex<f32>], cycles_per_sample: f64) {
    let middle = samples.len() as f64 / 2.0;
    for (index, sample) in samples.iter_mut().enumerate() {
        let turns = (cycles_per_sample * (index as f64 - middle)).fract();
        let (sin, cos) = (TAU * turns).sin_cos();
        *sample *= Complex::new(cos as f32, sin as f32);
    }
}

fn clipped(buffers: &CaptureBuffers) -> Option<u8> {
    buffers
        .lanes
        .iter()
        .position(|lane| {
            lane.iter()
                .any(|sample| sample.re.abs() >= CLIP_LEVEL || sample.im.abs() >= CLIP_LEVEL)
        })
        .map(|lane| lane as u8)
}

fn same_device(buffers: &CaptureBuffers, a: usize, b: usize) -> bool {
    buffers.devices.get(a) == buffers.devices.get(b)
}

fn coarse_failure(error: CoarseError, lane: usize) -> SolveFailure {
    let lane = lane as u8;
    match error {
        CoarseError::NoPeak => SolveFailure::NoPeak { lane },
        CoarseError::Ambiguous => SolveFailure::Ambiguous { lane },
        CoarseError::Short
        | CoarseError::Rate
        | CoarseError::LaneCount { .. }
        | CoarseError::LaneLength => SolveFailure::Short,
    }
}

fn bin_failure(error: BinError) -> SolveFailure {
    match error {
        BinError::LowCoherence { lane, coherence } => SolveFailure::LowCoherence {
            lane: lane as u8,
            coherence,
        },
        BinError::FewBins => SolveFailure::FewBins,
        BinError::Short | BinError::LaneCount { .. } => SolveFailure::Short,
    }
}

#[cfg(test)]
mod tests;
