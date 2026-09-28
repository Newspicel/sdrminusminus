use num_complex::Complex;
use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_device::{GapScope, LaneMark, Uncertainty};
use sdrmm_wire::{ArrayOrientation, ArrayTuningMode, Coherence};

use super::*;

const RATE: f64 = 2_400_000.0;
const BLOCK: usize = 16_384;

fn frame(lanes: usize, devices: &[u8]) -> LiveFrame {
    let mut map = [0u8; MAX_LANES];
    map[..devices.len()].copy_from_slice(devices);
    LiveFrame {
        sample_rate: RATE,
        center_hz: 433.92e6,
        lane_centers_hz: vec![433.92e6; lanes],
        orientation: ArrayOrientation::default(),
        tier: Coherence::TimeSync,
        keeps_phase: false,
        needs_time: true,
        tuning: ArrayTuningMode::Together,
        dc_block: false,
        in_flight: 0,
        devices: map,
    }
}

struct Feed {
    windows: Windows,
    frame: LiveFrame,
    board: StatusBoard,
    index: u64,
    events: Vec<AggregatorEvent>,
}

impl Feed {
    fn new(lanes: usize, devices: &[u8]) -> Self {
        Self {
            windows: Windows::new(lanes, RATE),
            frame: frame(lanes, devices),
            board: StatusBoard::new(lanes),
            index: 0,
            events: Vec::new(),
        }
    }

    fn block(&mut self, notes: &[AlignNote], power: impl Fn(u64) -> f32) -> Observation {
        let mut held = AlignNotes::new();
        for note in notes {
            held.push(*note);
        }
        let lanes: Vec<Vec<Complex<f32>>> = (0..self.frame.lanes())
            .map(|_| {
                (0..BLOCK as u64)
                    .map(|at| Complex::new(power(self.index + at).sqrt(), 0.0))
                    .collect()
            })
            .collect();
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let seen = self
            .windows
            .observe(&held, &views, self.index, &self.frame, &self.board);
        let events = &mut self.events;
        self.windows.drain_events(|event| events.push(event));
        self.index += BLOCK as u64;
        seen
    }
}

fn mark(at: u64, mark: LaneMark) -> AlignNote {
    AlignNote::Mark { lane: 0, at, mark }
}

const QUIET: f32 = 1e-4;
const LOUD: f32 = 1e-2;

#[test]
fn noise_onset_is_found_by_power_after_the_mark() {
    let mut feed = Feed::new(2, &[0, 0]);
    let step = 26_000;
    let power = |at: u64| if at < step { QUIET } else { LOUD };
    feed.block(&[], power);
    let on = mark(
        20_000,
        LaneMark::NoiseSource {
            on: true,
            in_flight: 8_192,
        },
    );
    let seen = feed.block(&[on], power);
    feed.block(&[], power);
    assert_eq!(feed.events, [AggregatorEvent::NoiseOnset { at: 25_600 }]);
    assert_eq!(seen.noise_began, Some(25_600));
    assert_eq!(
        feed.windows.reference(),
        Some((25_600 + ONSET_SETTLE, u64::MAX))
    );
    assert_eq!(
        feed.windows.gate_at(20_000 - PRE_GUARD),
        Some(ProcessorGate::Calibrating)
    );
    assert_eq!(feed.windows.gate_at(20_000 - PRE_GUARD - 1), None);
}

#[test]
fn a_mark_without_a_power_step_is_noise_not_seen() {
    let mut feed = Feed::new(2, &[0, 0]);
    feed.block(&[], |_| QUIET);
    let on = mark(
        20_000,
        LaneMark::NoiseSource {
            on: true,
            in_flight: 8_192,
        },
    );
    feed.block(&[on], |_| QUIET);
    while feed.index < 200_000 {
        feed.block(&[], |_| QUIET);
    }
    assert_eq!(feed.events, [AggregatorEvent::NoiseNotSeen]);
    assert_eq!(feed.windows.reference(), None);
    assert_eq!(
        feed.windows.gate_at(feed.index),
        Some(ProcessorGate::Calibrating)
    );
}

#[test]
fn the_gate_covers_the_uncertain_tail_after_off() {
    let mut feed = Feed::new(2, &[0, 0]);
    let (step_on, step_off) = (26_000, 90_000);
    let power = |at: u64| {
        if (step_on..step_off).contains(&at) {
            LOUD
        } else {
            QUIET
        }
    };
    feed.block(&[], power);
    feed.block(
        &[mark(
            20_000,
            LaneMark::NoiseSource {
                on: true,
                in_flight: 8_192,
            },
        )],
        power,
    );
    while feed.index < 81_920 {
        feed.block(&[], power);
    }
    let off = mark(
        85_000,
        LaneMark::NoiseSource {
            on: false,
            in_flight: 8_192,
        },
    );
    feed.block(&[off], power);
    feed.block(&[], power);
    let ended = 90_112;
    assert!(
        feed.events
            .contains(&AggregatorEvent::NoiseEnded { at: ended })
    );
    assert_eq!(
        feed.windows.gate_at(ended + ONSET_SETTLE - 1),
        Some(ProcessorGate::Calibrating)
    );
    assert_eq!(feed.windows.gate_at(ended + ONSET_SETTLE), None);
    let (from, until) = feed.windows.reference().expect("a reference window");
    assert_eq!(from, 25_600 + ONSET_SETTLE);
    assert_eq!(until, 85_000);
}

#[test]
fn an_unisolated_source_gates_until_in_flight_passes() {
    let mut feed = Feed::new(2, &[0, 0]);
    let power = |at: u64| if at >= 26_000 { LOUD } else { QUIET };
    feed.block(&[], power);
    feed.block(
        &[mark(
            20_000,
            LaneMark::NoiseSource {
                on: true,
                in_flight: 8_192,
            },
        )],
        power,
    );
    while feed.index < 81_920 {
        feed.block(&[], power);
    }
    feed.block(
        &[mark(
            85_000,
            LaneMark::NoiseSource {
                on: false,
                in_flight: 8_192,
            },
        )],
        power,
    );
    feed.block(&[], power);
    assert!(
        feed.events
            .contains(&AggregatorEvent::NoiseEnded { at: 93_192 })
    );
    assert_eq!(
        feed.windows.gate_at(93_191),
        Some(ProcessorGate::Calibrating)
    );
    assert_eq!(feed.windows.gate_at(93_192), None);
}

#[test]
fn a_retune_blanks_in_flight_plus_settle() {
    let mut feed = Feed::new(2, &[0, 0]);
    let in_flight = 10_000;
    let settle = (PLL_SETTLE_S * RATE) as u64;
    feed.block(&[], |_| QUIET);
    let seen = feed.block(&[mark(20_000, LaneMark::Retuned { in_flight })], |_| QUIET);
    assert_eq!(seen.blank_began, Some(BlankCause::Retune));
    let end = 20_000 + in_flight + settle;
    assert_eq!(
        feed.windows.gate_at(20_000 - PRE_GUARD),
        Some(ProcessorGate::Retuning)
    );
    assert_eq!(feed.windows.gate_at(end - 1), Some(ProcessorGate::Retuning));
    assert_eq!(feed.windows.gate_at(end), None);
    assert_eq!(feed.windows.next_change(20_000), Some(end));
    let mut ended = None;
    while feed.index < end + BLOCK as u64 {
        let seen = feed.block(&[], |_| QUIET);
        ended = ended.or(seen.blank_ended);
    }
    assert_eq!(ended, Some(BlankCause::Retune));
    assert!(feed.events.contains(&AggregatorEvent::BlankEnded {
        cause: BlankCause::Retune,
        at: end
    }));
    feed.windows.prune(end);
    assert_eq!(feed.windows.gate_at(end - 1), None);
}

#[test]
fn a_gain_step_during_a_retune_blank_makes_it_a_gain_blank() {
    let mut feed = Feed::new(2, &[0, 0]);
    feed.block(&[], |_| QUIET);
    let seen = feed.block(
        &[
            mark(20_000, LaneMark::Retuned { in_flight: 100 }),
            mark(20_500, LaneMark::GainChanged { in_flight: 100 }),
        ],
        |_| QUIET,
    );
    assert_eq!(seen.blank_began, Some(BlankCause::Gain));
}

#[test]
fn a_device_wide_gap_on_a_one_device_array_does_not_resync() {
    let note = AlignNote::Uncertain {
        lane: 1,
        at: 0,
        error: 5,
        scope: GapScope::Device,
        cause: Uncertainty::Overflow,
    };
    let mut single = Feed::new(3, &[0, 0, 0]);
    let seen = single.block(&[note], |_| QUIET);
    assert_eq!(seen.lost, 0);
    assert_eq!(seen.reset, Some(ResetCause::Gap));
    assert!(single.events.is_empty());
    assert_eq!(single.board.sync(), SyncState::Idle);

    let mut split = Feed::new(3, &[0, 1, 1]);
    let seen = split.block(&[note], |_| QUIET);
    assert_eq!(seen.lost, 0b110);
    assert_eq!(split.board.sync(), SyncState::Lost);
    assert_eq!(
        split.events,
        [
            AggregatorEvent::Uncertain {
                lane: 1,
                error: 5,
                scope: GapScope::Device
            },
            AggregatorEvent::Uncertain {
                lane: 2,
                error: 5,
                scope: GapScope::Device
            },
        ]
    );
}

#[test]
fn gaps_count_on_their_lane() {
    let mut feed = Feed::new(2, &[0, 0]);
    let seen = feed.block(
        &[AlignNote::Gap {
            lane: 1,
            at: 0,
            missing: 77,
        }],
        |_| QUIET,
    );
    assert!(seen.discontinuity);
    let mut status = sdrmm_wire::ArrayStatus::default();
    feed.board.fill(&mut status);
    assert_eq!(status.lanes[1].gaps, 1);
    assert_eq!(status.lanes[1].gap_samples, 77);
    assert_eq!(status.lanes[0].gaps, 0);
}

#[test]
fn a_mark_in_the_first_block_takes_its_baseline_before_the_mark() {
    let mut feed = Feed::new(2, &[0, 0]);
    let power = |at: u64| if at < 26_000 { QUIET } else { LOUD };
    let on = mark(
        20_000,
        LaneMark::NoiseSource {
            on: true,
            in_flight: 8_192,
        },
    );
    feed.block(&[on], power);
    feed.block(&[], power);
    assert_eq!(feed.events, [AggregatorEvent::NoiseOnset { at: 25_600 }]);
}

#[test]
fn a_mark_with_no_quiet_samples_before_it_is_noise_not_seen() {
    let mut feed = Feed::new(2, &[0, 0]);
    let on = mark(
        512,
        LaneMark::NoiseSource {
            on: true,
            in_flight: 0,
        },
    );
    feed.block(&[on], |_| LOUD);
    assert_eq!(feed.events, [AggregatorEvent::NoiseNotSeen]);
}

#[test]
fn noise_switched_on_again_while_ending_keeps_the_gate_closed() {
    let mut feed = Feed::new(2, &[0, 0]);
    let power = |at: u64| if at >= 26_000 { LOUD } else { QUIET };
    feed.block(&[], power);
    let on = |at| {
        mark(
            at,
            LaneMark::NoiseSource {
                on: true,
                in_flight: 8_192,
            },
        )
    };
    feed.block(&[on(20_000)], power);
    while feed.index < 81_920 {
        feed.block(&[], power);
    }
    let off = mark(
        85_000,
        LaneMark::NoiseSource {
            on: false,
            in_flight: 8_192,
        },
    );
    feed.block(&[off, on(88_000)], power);
    while feed.index < 150_000 {
        feed.block(&[], power);
    }
    for at in [20_000 - PRE_GUARD, 85_000, 93_192, 120_000, feed.index] {
        assert_eq!(
            feed.windows.gate_at(at),
            Some(ProcessorGate::Calibrating),
            "{at}"
        );
    }
}
