use num_complex::Complex;
use sdrmm_dsp::xcorr::XCorr;
use sdrmm_wire::{CalParams, CalState, Coherence, LaneCal};

/// The most a lane may be pulled back to line up with lane zero. A shared clock puts real arrays
/// well inside this; anything beyond it is a wiring fault, not a calibration.
const MAX_CORRECTION: usize = 4_096;
/// How much of the previous solution survives each new one. Slow enough that a passing signal
/// cannot yank the array, fast enough to follow a front end warming up.
const TRACK: f32 = 0.2;
/// Below this the lanes are not looking at the same thing and the solution is left alone.
const USABLE_COHERENCE: f32 = 0.2;

const MIN_FRAME: usize = 1_024;
const MAX_FRAME: usize = 32_768;

#[derive(Clone, Copy)]
struct Lane {
    delay: f32,
    gain: f32,
    phase: Complex<f32>,
    quality: f32,
}

impl Lane {
    const fn identity() -> Self {
        Self {
            delay: 0.0,
            gain: 1.0,
            phase: Complex::new(1.0, 0.0),
            quality: 0.0,
        }
    }

    fn weight(&self) -> Complex<f32> {
        self.phase * self.gain
    }
}

/// Solves what separates each lane from lane zero, and applies the answer.
///
/// One measurement covers both tiers: delay is meaningful wherever the clock is shared, phase
/// only where the synthesizer is too. What differs is what the result is allowed to be used for,
/// which is why the state it publishes says so out loud rather than leaving a caller to assume.
pub(crate) struct Calibrator {
    lanes: usize,
    params: CalParams,
    xcorr: XCorr,
    frame: usize,
    solutions: Vec<Lane>,
    history: Vec<Vec<Complex<f32>>>,
    corrected: Vec<Vec<Complex<f32>>>,
    state: CalState,
    pending: bool,
    solved: bool,
    phase_solved: bool,
}

fn frame_for(sample_rate: f64, bandwidth_hz: f64) -> usize {
    let wanted = (sample_rate / bandwidth_hz.max(1.0) * 512.0) as usize;
    wanted.clamp(MIN_FRAME, MAX_FRAME).next_power_of_two()
}

impl Calibrator {
    pub(crate) fn new(lanes: usize, tier: Coherence, params: CalParams, sample_rate: f64) -> Self {
        let frame = frame_for(sample_rate, params.bandwidth_hz);
        Self {
            lanes,
            params,
            xcorr: XCorr::new(frame),
            frame,
            solutions: vec![Lane::identity(); lanes],
            history: vec![vec![Complex::default(); MAX_CORRECTION]; lanes],
            corrected: vec![Vec::new(); lanes],
            state: CalState {
                tier,
                lanes: vec![LaneCal::default(); lanes],
                phase_unknown: !tier.has_phase(),
                solved: false,
            },
            pending: true,
            solved: false,
            phase_solved: false,
        }
    }

    pub(crate) fn apply(&mut self, params: CalParams, sample_rate: f64) {
        let frame = frame_for(sample_rate, params.bandwidth_hz);
        if frame != self.frame {
            self.xcorr = XCorr::new(frame);
            self.frame = frame;
        }
        self.params = params;
        self.refresh_phase_state();
    }

    /// Throws the solution away, which is what a retune on a time-synced array and an operator
    /// pressing calibrate both mean.
    pub(crate) fn invalidate(&mut self, keep_delay: bool) {
        for lane in &mut self.solutions {
            if keep_delay {
                lane.phase = Complex::new(1.0, 0.0);
            } else {
                *lane = Lane::identity();
            }
            lane.quality = 0.0;
        }
        self.solved = false;
        self.phase_solved = false;
        self.pending = true;
        self.publish();
        self.refresh_phase_state();
    }

    /// Whether the lanes are currently being fed something whose phase carries hardware offsets
    /// and nothing else.
    ///
    /// A signal arriving over the air brings a bearing with it, and no measurement can separate
    /// that from the receiver's own phase. Removing it would be removing the answer, so phase is
    /// only ever solved against an injected reference or a declared pilot.
    const fn phase_reference(&self) -> bool {
        matches!(self.params.source, sdrmm_wire::CalSource::Noise) || self.params.pilot_hz.is_some()
    }

    pub(crate) fn state(&self) -> &CalState {
        &self.state
    }

    /// Whether processors that need inter-lane phase may run at all.
    pub(crate) const fn phase_usable(&self) -> bool {
        !self.state.phase_unknown
    }

    fn refresh_phase_state(&mut self) {
        self.state.phase_unknown = match self.state.tier {
            Coherence::PhaseCoherent => false,
            Coherence::TimeSync => !self.phase_solved,
            Coherence::None => true,
        };
        self.state.solved = self.solved;
    }

    /// Measures, then corrects. The measurement runs on what arrived, so a correction never
    /// feeds back into the estimate that produced it.
    pub(crate) fn process(&mut self, lanes: &[&[Complex<f32>]]) {
        let count = lanes.len().min(self.lanes);
        if count == 0 {
            return;
        }
        if (self.pending || self.params.track) && lanes[0].len() >= self.frame {
            self.solve(&lanes[..count]);
        }
        self.correct(&lanes[..count]);
    }

    fn solve(&mut self, lanes: &[&[Complex<f32>]]) {
        let reference = lanes[0];
        let mut worst = f32::MAX;
        for (lane, source) in lanes.iter().enumerate().skip(1) {
            let estimate = self.xcorr.estimate(reference, source);
            if estimate.coherence < USABLE_COHERENCE {
                self.solutions[lane].quality = estimate.coherence;
                worst = 0.0;
                continue;
            }
            let delay = estimate.delay_samples.clamp(
                -(MAX_CORRECTION as f32) / 2.0,
                (MAX_CORRECTION as f32) / 2.0,
            );
            let gain = if estimate.gain > f32::MIN_POSITIVE {
                1.0 / estimate.gain
            } else {
                1.0
            };
            let blend = if self.pending { 1.0 } else { TRACK };
            let phase_reference = self.phase_reference();
            let solution = &mut self.solutions[lane];
            solution.delay = solution.delay * (1.0 - blend) + delay * blend;
            solution.gain = solution.gain * (1.0 - blend) + gain * blend;
            if phase_reference {
                let target = Complex::from_polar(1.0, -estimate.phase_rad);
                let blended = solution.phase * (1.0 - blend) + target * blend;
                solution.phase = blended / blended.norm().max(f32::MIN_POSITIVE);
            }
            solution.quality = estimate.coherence;
            worst = worst.min(estimate.coherence);
        }
        self.solutions[0] = Lane {
            quality: 1.0,
            ..Lane::identity()
        };
        if worst >= USABLE_COHERENCE {
            self.solved = true;
            self.pending = false;
            self.phase_solved |= self.phase_reference();
        }
        self.publish();
        self.refresh_phase_state();
    }

    fn publish(&mut self) {
        for (slot, lane) in self.state.lanes.iter_mut().zip(&self.solutions) {
            slot.delay_samples = lane.delay;
            slot.phase_deg = lane.phase.arg().to_degrees();
            slot.gain_db = 20.0 * lane.gain.max(1e-6).log10();
            slot.quality = lane.quality;
        }
    }

    /// Lines the lanes up and rotates them onto lane zero.
    ///
    /// Delays are relative, so the whole array is shifted until every correction looks backwards
    /// into samples that have already arrived rather than forwards into ones that have not.
    ///
    /// Lanes cut from one stream of one radio have no delay between them to correct, and a
    /// measurement of a delay that cannot exist is noise: shifting by it would be the only thing
    /// that ever put them out of step. The measurement is still published, as a diagnostic.
    fn correct(&mut self, lanes: &[&[Complex<f32>]]) {
        let phase_usable = self.phase_usable();
        let correct_delay = !self.state.tier.has_phase();
        let latest = if correct_delay {
            self.solutions
                .iter()
                .take(lanes.len())
                .map(|lane| lane.delay)
                .fold(f32::MIN, f32::max)
        } else {
            0.0
        };
        for (index, source) in lanes.iter().enumerate() {
            let measured = if correct_delay {
                self.solutions[index].delay
            } else {
                0.0
            };
            let shift = ((latest - measured).round().max(0.0) as usize).min(MAX_CORRECTION);
            let weight = if phase_usable {
                self.solutions[index].weight()
            } else {
                Complex::new(self.solutions[index].gain, 0.0)
            };
            let history = &self.history[index];
            let out = &mut self.corrected[index];
            out.clear();
            out.reserve(source.len());
            for position in 0..source.len() {
                let sample = if position >= shift {
                    source[position - shift]
                } else {
                    history[history.len() + position - shift]
                };
                out.push(sample * weight);
            }
        }
        for (index, source) in lanes.iter().enumerate() {
            let history = &mut self.history[index];
            let keep = source.len().min(MAX_CORRECTION);
            history.rotate_left(keep);
            let start = history.len() - keep;
            history[start..].copy_from_slice(&source[source.len() - keep..]);
        }
    }

    pub(crate) fn with_lanes<R>(&self, count: usize, f: impl FnOnce(&[&[Complex<f32>]]) -> R) -> R {
        let mut view: [&[Complex<f32>]; sdrmm_wire::MAX_STREAMS as usize] =
            [&[]; sdrmm_wire::MAX_STREAMS as usize];
        let lanes = self.corrected.len().min(view.len());
        for (slot, lane) in view.iter_mut().zip(&self.corrected) {
            *slot = &lane[..count.min(lane.len())];
        }
        f(&view[..lanes])
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    const RATE: f64 = 1_000_000.0;

    fn params() -> CalParams {
        CalParams {
            bandwidth_hz: 200_000.0,
            ..CalParams::default()
        }
    }

    fn injected() -> CalParams {
        CalParams {
            source: sdrmm_wire::CalSource::Noise,
            ..params()
        }
    }

    fn tone(len: usize, offset: usize, rotation: Complex<f32>) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                let t = (k + offset) as f32;
                let phase = TAU * (0.031 * t + 0.000_02 * t * t);
                Complex::from_polar(1.0, phase) * rotation
            })
            .collect()
    }

    fn drive(cal: &mut Calibrator, blocks: usize, build: impl Fn(usize) -> Vec<Vec<Complex<f32>>>) {
        for round in 0..blocks {
            let lanes = build(round);
            let view: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
            cal.process(&view);
        }
    }

    #[test]
    fn a_phase_coherent_array_solves_the_rotation_between_its_lanes() {
        let mut cal = Calibrator::new(2, Coherence::PhaseCoherent, injected(), RATE);
        let rotation = Complex::from_polar(0.5f32, 1.2);
        drive(&mut cal, 3, |round| {
            let base = round * 4_096;
            vec![
                tone(4_096, base, Complex::new(1.0, 0.0)),
                tone(4_096, base, rotation),
            ]
        });
        let state = cal.state();
        assert!(state.solved, "{state:?}");
        assert!(!state.phase_unknown, "{state:?}");
        assert!(
            (state.lanes[1].phase_deg + 1.2f32.to_degrees()).abs() < 3.0,
            "{:?}",
            state.lanes[1]
        );
        assert!(
            (state.lanes[1].gain_db - 6.0).abs() < 1.0,
            "{:?}",
            state.lanes[1]
        );
        assert!(state.lanes[1].quality > 0.9, "{:?}", state.lanes[1]);
    }

    #[test]
    fn correction_puts_the_lanes_on_top_of_each_other() {
        let mut cal = Calibrator::new(2, Coherence::TimeSync, injected(), RATE);
        let rotation = Complex::from_polar(0.5f32, 1.2);
        let shift = 7usize;
        drive(&mut cal, 4, |round| {
            let base = round * 4_096;
            vec![
                tone(4_096, base + shift, Complex::new(1.0, 0.0)),
                tone(4_096, base, rotation),
            ]
        });
        let matched = cal.with_lanes(4_096, |lanes| {
            let window = 1_024..3_072;
            let mut worst = 0.0f32;
            for index in window {
                worst = worst.max((lanes[0][index] - lanes[1][index]).norm());
            }
            worst
        });
        assert!(matched < 0.1, "lanes still differ by {matched}");
    }

    #[test]
    fn a_time_synced_array_without_a_pilot_refuses_to_trust_its_phase() {
        let mut cal = Calibrator::new(2, Coherence::TimeSync, params(), RATE);
        drive(&mut cal, 3, |round| {
            let base = round * 4_096;
            vec![
                tone(4_096, base, Complex::new(1.0, 0.0)),
                tone(4_096, base, Complex::from_polar(1.0, 2.0)),
            ]
        });
        assert!(cal.state().solved);
        assert!(cal.state().phase_unknown);
        assert!(!cal.phase_usable());
        let residual = cal.with_lanes(4_096, |lanes| lanes[1][2_048]);
        assert!(residual.norm() > 0.5, "the samples still have to flow");
    }

    #[test]
    fn a_pilot_lets_a_time_synced_array_use_its_phase() {
        let mut cal = Calibrator::new(
            2,
            Coherence::TimeSync,
            CalParams {
                pilot_hz: Some(25_000.0),
                ..params()
            },
            RATE,
        );
        drive(&mut cal, 3, |round| {
            let base = round * 4_096;
            vec![
                tone(4_096, base, Complex::new(1.0, 0.0)),
                tone(4_096, base, Complex::from_polar(1.0, 2.0)),
            ]
        });
        assert!(!cal.state().phase_unknown, "{:?}", cal.state());
    }

    #[test]
    fn lanes_that_share_nothing_leave_the_solution_unsolved() {
        let mut cal = Calibrator::new(2, Coherence::PhaseCoherent, params(), RATE);
        drive(&mut cal, 3, |round| {
            let mut state = 0x1234u64 ^ (round as u64) << 20;
            let mut next = move || {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;
                (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32 - 1.0
            };
            (0..2)
                .map(|_| {
                    (0..4_096)
                        .map(|_| Complex::new(next(), next()))
                        .collect::<Vec<_>>()
                })
                .collect()
        });
        assert!(!cal.state().solved);
        assert!(cal.state().lanes[1].quality < USABLE_COHERENCE);
    }

    #[test]
    fn invalidating_keeps_delay_when_only_the_synthesizer_moved() {
        let mut cal = Calibrator::new(2, Coherence::TimeSync, params(), RATE);
        drive(&mut cal, 3, |round| {
            let base = round * 4_096;
            vec![
                tone(4_096, base + 9, Complex::new(1.0, 0.0)),
                tone(4_096, base, Complex::new(1.0, 0.0)),
            ]
        });
        let delay = cal.state().lanes[1].delay_samples;
        assert!(delay.abs() > 1.0, "no delay was measured: {delay}");
        cal.invalidate(true);
        assert_eq!(cal.state().lanes[1].delay_samples, delay);
        assert!(!cal.state().solved);
        cal.invalidate(false);
        assert_eq!(cal.state().lanes[1].delay_samples, 0.0);
    }
}
