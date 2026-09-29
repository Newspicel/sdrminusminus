use crate::{FatalHandle, RxSink, Sample};

/// Splits the lanes of one interleaved buffer across the sinks that asked for them.
///
/// The capture path carries one sink, so a radio whose lanes share a buffer hands over a sink
/// that de-interleaves. A gap the supervisor reports as a jump in the sample index is divided
/// back out per lane, so every lane stays on the same timeline as its neighbours.
pub fn fan_out(sinks: Vec<RxSink>, lane_samples: usize) -> RxSink {
    let lanes = sinks.len();
    if lanes <= 1 {
        return sinks
            .into_iter()
            .next()
            .unwrap_or_else(|| RxSink::new(|_, _| {}));
    }
    let mut sinks = sinks;
    let failures: Vec<FatalHandle> = sinks.iter_mut().map(RxSink::share_failure).collect();
    let mut lane_buffers: Vec<Vec<Sample>> = (0..lanes)
        .map(|_| Vec::with_capacity(lane_samples))
        .collect();
    let mut expected: Option<u64> = None;
    RxSink::with_fatal_handler(
        move |samples, index| {
            if let Some(expected) = expected.filter(|expected| index > *expected) {
                let lost = (index - expected) / lanes as u64;
                for sink in &mut sinks {
                    sink.dropped(lost);
                }
            }
            expected = Some(index + samples.len() as u64);
            for lane in &mut lane_buffers {
                lane.clear();
            }
            for (slot, sample) in samples.iter().enumerate() {
                lane_buffers[slot % lanes].push(*sample);
            }
            for (sink, lane) in sinks.iter_mut().zip(&lane_buffers) {
                sink.push(lane);
            }
        },
        move |error| {
            for failure in &failures {
                failure.fail(error.clone());
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::DeviceError;

    fn recording() -> (RxSink, mpsc::Receiver<(u64, Vec<f32>)>) {
        let (tx, rx) = mpsc::channel();
        (
            RxSink::new(move |samples: &[Sample], index| {
                let _ = tx.send((index, samples.iter().map(|s| s.re).collect()));
            }),
            rx,
        )
    }

    #[test]
    fn one_sink_is_handed_straight_through() {
        let (sink, seen) = recording();
        let mut sink = fan_out(vec![sink], 4);
        sink.push(&[Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)]);
        assert_eq!(seen.try_recv().expect("pushed"), (0, vec![1.0, 2.0]));
    }

    #[test]
    fn two_lanes_are_split_apart_and_each_keeps_its_own_count() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second], 4);
        let block: Vec<Sample> = (1..=6).map(|n| Sample::new(n as f32, 0.0)).collect();
        sink.push(&block);
        sink.push(&block);
        assert_eq!(left.try_recv().expect("lane 0"), (0, vec![1.0, 3.0, 5.0]));
        assert_eq!(right.try_recv().expect("lane 1"), (0, vec![2.0, 4.0, 6.0]));
        assert_eq!(left.try_recv().expect("lane 0"), (3, vec![1.0, 3.0, 5.0]));
        assert_eq!(right.try_recv().expect("lane 1"), (3, vec![2.0, 4.0, 6.0]));
    }

    #[test]
    fn a_gap_the_supervisor_reports_moves_every_lane_by_its_own_share() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second], 4);
        let block = [Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)];
        sink.push(&block);
        sink.dropped(100);
        sink.push(&block);
        assert_eq!(left.try_recv().expect("lane 0").0, 0);
        assert_eq!(right.try_recv().expect("lane 1").0, 0);
        assert_eq!(
            left.try_recv().expect("lane 0").0,
            51,
            "one sample delivered plus half the interleaved gap"
        );
        assert_eq!(right.try_recv().expect("lane 1").0, 51);
    }

    #[test]
    fn a_fault_reaches_every_lane_with_the_kind_it_had() {
        let (faults, seen) = mpsc::channel();
        let sinks: Vec<RxSink> = (0..3)
            .map(|lane| {
                let faults = faults.clone();
                RxSink::with_fatal_handler(
                    |_, _| {},
                    move |error| {
                        let _ = faults.send((lane, error.to_string()));
                    },
                )
            })
            .collect();
        let mut sink = fan_out(sinks, 4);
        sink.fail(DeviceError::Disconnected("unplugged".to_string()));
        let mut told: Vec<usize> = seen
            .try_iter()
            .map(|(lane, why)| {
                assert!(why.contains("no longer attached"), "{why}");
                lane
            })
            .collect();
        told.sort_unstable();
        assert_eq!(told, vec![0, 1, 2]);
    }

    #[test]
    fn no_sinks_at_all_still_yields_something_that_can_be_pushed_to() {
        let mut sink = fan_out(Vec::new(), 4);
        sink.push(&[Sample::new(1.0, 0.0)]);
    }
}
