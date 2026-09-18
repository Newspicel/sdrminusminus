use super::*;
use crate::placement::{Limits, allocate, heuristic, received};

fn unlimited() -> Limits {
    Limits {
        duration: Duration::from_secs(10),
        nodes: usize::MAX,
    }
}

fn oracle_lane_choices(decoder: &Placeable, radios: &[Radio]) -> Vec<Lane> {
    let choices: Vec<_> = decoder
        .lanes
        .iter()
        .copied()
        .filter(|lane| {
            radios.iter().any(|radio| {
                radio.device_set == lane.device_set
                    && lane.stream < radio.capabilities.rx_streams.max(1)
            })
        })
        .collect();
    if decoder.pinned && decoder.held.is_some_and(|held| choices.contains(&held)) {
        return vec![decoder.held.unwrap()];
    }
    choices
}

fn oracle_radio(radio: &Radio, channels: &[ChannelInfo]) -> usize {
    let streams: Vec<_> = if radio.capabilities.per_stream.tuning {
        (0..radio.capabilities.rx_streams).collect()
    } else {
        vec![0]
    };
    streams
        .into_iter()
        .map(|stream| {
            let channels: Vec<_> = channels
                .iter()
                .filter(|channel| !radio.capabilities.per_stream.tuning || channel.stream == stream)
                .collect();
            let current = center_of(&radio.settings, stream, &radio.capabilities.per_stream);
            let rate = sample_rate_of(&radio.settings);
            let mut centers = vec![current];
            if radio.tunes_freely
                && radio
                    .settings
                    .for_stream(stream, &radio.capabilities.per_stream)
                    .tunes_itself()
            {
                centers.extend(channels.iter().flat_map(|channel| {
                    let (low, high) = sdrmm_channels::occupied_band(&channel.settings.params);
                    [
                        channel.settings.frequency_hz + high - rate / 2.0,
                        channel.settings.frequency_hz + low + rate / 2.0,
                    ]
                }));
                centers.extend(
                    radio
                        .capabilities
                        .freq_ranges
                        .iter()
                        .flat_map(|range| [range.min, range.max]),
                );
                centers.retain(|hz| {
                    radio.capabilities.freq_ranges.is_empty()
                        || radio
                            .capabilities
                            .freq_ranges
                            .iter()
                            .any(|range| range.min <= *hz && *hz <= range.max)
                });
            }
            centers
                .into_iter()
                .map(|center| {
                    channels
                        .iter()
                        .filter(|channel| {
                            let (low, high) =
                                sdrmm_channels::occupied_band(&channel.settings.params);
                            let offset = channel.settings.frequency_hz - center;
                            offset + low >= -rate / 2.0 && offset + high <= rate / 2.0
                        })
                        .count()
                })
                .max()
                .unwrap_or(0)
        })
        .sum()
}

fn oracle_score(decoders: &[Placeable], radios: &[Radio], placements: &[Placement]) -> usize {
    radios
        .iter()
        .map(|radio| {
            let mut channels = radio.fixed.clone();
            for decoder in decoders {
                if let Some(placement) = placements.iter().find(|placement| {
                    placement.node == decoder.node && placement.lane.device_set == radio.device_set
                }) {
                    channels.push(ChannelInfo {
                        node: Some(decoder.node.clone()),
                        stream: placement.lane.stream,
                        settings: decoder.settings.clone(),
                        ..parked(0, 0.0)
                    });
                }
            }
            oracle_radio(radio, &channels)
        })
        .sum()
}

fn exhaustive(decoders: &[Placeable], radios: &[Radio]) -> usize {
    fn visit(
        decoders: &[Placeable],
        radios: &[Radio],
        index: usize,
        placed: &mut Vec<Placement>,
    ) -> usize {
        if index == decoders.len() {
            return oracle_score(decoders, radios, placed);
        }
        let decoder = &decoders[index];
        let choices = oracle_lane_choices(decoder, radios);
        if choices.is_empty() {
            return visit(decoders, radios, index + 1, placed);
        }
        choices
            .into_iter()
            .map(|lane| {
                placed.push(Placement {
                    node: decoder.node.clone(),
                    lane,
                });
                let score = visit(decoders, radios, index + 1, placed);
                placed.pop();
                score
            })
            .max()
            .unwrap_or(0)
    }
    visit(decoders, radios, 0, &mut Vec::new())
}

struct Random(u64);

impl Random {
    fn next(&mut self, max: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.0 >> 32) % max
    }
}

fn scenario(seed: u64, count: usize, radio_count: u32) -> (Vec<Radio>, Vec<Placeable>) {
    let mut random = Random(seed + 1);
    let mut radios: Vec<_> = (1..=radio_count)
        .map(|id| {
            let mut radio = radio(id, tuned(100e6));
            radio.settings.sample_rate = Some(1e6 + random.next(4) as f64 * 1e6);
            radio.capabilities.rx_streams = if seed.is_multiple_of(5) { 2 } else { 1 };
            radio.capabilities.per_stream.tuning = seed.is_multiple_of(10);
            if random.next(5) == 0 {
                radio.settings.tuning = Some(Tuning::Manual);
            }
            radio
        })
        .collect();
    if seed.is_multiple_of(7) {
        radios[0].fixed.push(parked(99, 3e6));
    }
    if seed.is_multiple_of(11) {
        radios[0].capabilities.freq_ranges = vec![sdrmm_wire::Range {
            min: 98e6,
            max: 104e6,
            step: None,
        }];
    }
    let decoders = (0..count)
        .map(|index| {
            let frequency = 100e6 + random.next(16) as f64 * 750_000.0;
            let mut decoder = decoder(&format!("d{index}"), frequency, &[]);
            for radio in &radios {
                if random.next(3) != 0 {
                    decoder.lanes.push(Lane {
                        device_set: radio.device_set,
                        stream: random.next(u64::from(radio.capabilities.rx_streams)) as u32,
                    });
                }
            }
            if decoder.lanes.is_empty() {
                decoder.lanes.push(lane(1));
            }
            if random.next(2) == 0 {
                decoder.held = Some(decoder.lanes[0]);
                decoder.pinned = random.next(4) == 0;
            }
            if let ChannelParams::Nfm(params) = &mut decoder.settings.params {
                params.bandwidth_hz = if random.next(3) == 0 {
                    25_000.0
                } else {
                    12_500.0
                };
            }
            decoder
        })
        .collect();
    (radios, decoders)
}

#[test]
fn branch_and_bound_matches_independent_exhaustive_assignments() {
    let mut heuristic_misses = 0;
    let mut old_time = Duration::ZERO;
    let mut new_time = Duration::ZERO;
    let mut exhaustive_time = Duration::ZERO;
    for seed in 0..128 {
        let (radios, decoders) = scenario(seed, 6, 3);
        let started = Instant::now();
        let optimum = exhaustive(&decoders, &radios);
        exhaustive_time += started.elapsed();
        let started = Instant::now();
        let baseline = heuristic::place(&decoders, &radios);
        old_time += started.elapsed();
        let baseline_score = received(&decoders, &radios, &baseline);
        heuristic_misses += usize::from(baseline_score < optimum);
        let started = Instant::now();
        let result = allocate(&decoders, &radios, unlimited());
        new_time += started.elapsed();
        assert_eq!(
            received(&decoders, &radios, &result.placements),
            optimum,
            "seed {seed}"
        );
        assert_eq!(result.coverage.heard as usize, optimum, "seed {seed}");
        assert!(
            result.coverage.optimal(),
            "seed {seed}: {:?}",
            result.coverage
        );
        for placement in &result.placements {
            let decoder = decoders
                .iter()
                .find(|decoder| decoder.node == placement.node)
                .unwrap();
            assert!(oracle_lane_choices(decoder, &radios).contains(&placement.lane));
        }
    }
    println!(
        "128 small cases: heuristic={old_time:?}, branch_and_bound={new_time:?}, exhaustive={exhaustive_time:?}, heuristic_misses={heuristic_misses}"
    );
}

#[test]
fn exhausted_budgets_return_valid_placements_and_an_honest_bound() {
    for seed in 0..32 {
        let (radios, decoders) = scenario(seed, 6, 3);
        let optimum = exhaustive(&decoders, &radios);
        for limits in [
            Limits {
                duration: Duration::ZERO,
                nodes: usize::MAX,
            },
            Limits {
                duration: Duration::from_secs(1),
                nodes: 1,
            },
        ] {
            let result = allocate(&decoders, &radios, limits);
            assert_eq!(
                received(&decoders, &radios, &result.placements),
                result.coverage.heard as usize
            );
            assert!(result.coverage.heard as usize <= optimum);
            assert!(result.coverage.upper_bound as usize >= optimum);
            if result.coverage.optimal() {
                assert_eq!(result.coverage.heard as usize, optimum);
            }
        }
    }
}

#[test]
#[ignore = "allocation comparison benchmark"]
fn compares_realistic_sizes_without_truncating_decoder_sets() {
    for (count, radios_count) in [(32, 2), (32, 4), (96, 4)] {
        let (radios, decoders) = scenario(83, count, radios_count);
        let started = Instant::now();
        let baseline = heuristic::place(&decoders, &radios);
        let old_time = started.elapsed();
        let old_score = received(&decoders, &radios, &baseline);
        let started = Instant::now();
        let result = allocate(&decoders, &radios, Limits::default());
        let new_time = started.elapsed();
        assert_eq!(result.placements.len(), count);
        assert_eq!(
            received(&decoders, &radios, &result.placements),
            result.coverage.heard as usize
        );
        assert!(
            result.coverage.heard as usize >= old_score,
            "{count}/{radios_count}: {old_score} vs {:?}",
            result.coverage
        );
        assert!(
            new_time < Duration::from_secs(2),
            "{count}/{radios_count}: {new_time:?}"
        );
        println!(
            "{count} decoders/{radios_count} radios: heuristic={old_time:?} heard={old_score}, branch_and_bound={new_time:?} {:?}",
            result.coverage
        );
    }
}

#[test]
fn hundreds_of_decoders_are_never_truncated_to_one_machine_word() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders: Vec<_> = (0..256)
        .map(|i| {
            decoder(
                &format!("d{i}"),
                if i % 2 == 0 { 100e6 } else { 400e6 },
                &[A, B],
            )
        })
        .collect();
    let started = Instant::now();
    let result = allocate(&decoders, &radios, Limits::default());
    println!(
        "256 clustered decoders: {:?}, {:?}",
        started.elapsed(),
        result.coverage
    );
    assert_eq!(result.placements.len(), 256);
    assert_eq!(result.coverage.heard, 256);
    assert!(result.coverage.optimal());
    assert_eq!(received(&decoders, &radios, &result.placements), 256);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn large_wiring_graphs_return_a_feasible_bounded_result() {
    let (radios, decoders) = scenario(83, 256, 8);
    let started = Instant::now();
    let result = allocate(&decoders, &radios, Limits::default());
    let elapsed = started.elapsed();
    println!("256 varied decoders: {elapsed:?}, {:?}", result.coverage);
    assert_eq!(result.placements.len(), 256);
    assert_eq!(
        received(&decoders, &radios, &result.placements),
        result.coverage.heard as usize
    );
    assert!(elapsed < Duration::from_secs(2));
}

fn verify_exact(decoders: &[Placeable], radios: &[Radio]) {
    let result = allocate(decoders, radios, unlimited());
    let optimum = exhaustive(decoders, radios);
    assert_eq!(result.coverage.heard as usize, optimum);
    assert!(result.coverage.optimal());
    assert_eq!(received(decoders, radios, &result.placements), optimum);
}

#[test]
fn shared_tuning_does_not_invent_an_independent_window_per_stream() {
    let mut a = radio(A, tuned(100e6));
    a.capabilities.rx_streams = 2;
    let decoders = [
        decoder("low", 100e6, &[A]),
        Placeable {
            lanes: vec![Lane {
                device_set: A,
                stream: 1,
            }],
            ..decoder("high", 400e6, &[])
        },
    ];
    let result = allocate(&decoders, &[a], unlimited());
    assert_eq!(result.coverage.heard, 1);
    assert_eq!(result.coverage.upper_bound, 1);
}

#[test]
fn manual_streams_and_non_tunable_sources_remain_fixed() {
    let mut a = radio(A, tuned(100e6));
    a.capabilities.rx_streams = 2;
    a.capabilities.per_stream.tuning = true;
    a.settings.streams.push(sdrmm_wire::StreamSettings {
        stream: 0,
        center_hz: Some(100e6),
        tuning: Some(Tuning::Manual),
        gains: Vec::new(),
        antenna: None,
    });
    let mut b = radio(B, tuned(200e6));
    b.tunes_freely = false;
    let lanes = vec![
        lane(A),
        Lane {
            device_set: A,
            stream: 1,
        },
        lane(B),
    ];
    let decoders = [
        Placeable {
            lanes: lanes.clone(),
            ..decoder("low", 100e6, &[])
        },
        Placeable {
            lanes: lanes.clone(),
            ..decoder("middle", 200e6, &[])
        },
        Placeable {
            lanes,
            ..decoder("high", 400e6, &[])
        },
    ];
    verify_exact(&decoders, &[a, b]);
}

#[test]
fn disjoint_ranges_asymmetric_bands_and_impossible_bandwidths_match_the_oracle() {
    let mut a = radio(A, tuned(100e6));
    a.settings.sample_rate = Some(48_000.0);
    a.capabilities.freq_ranges = vec![
        sdrmm_wire::Range {
            min: 99e6,
            max: 100e6,
            step: None,
        },
        sdrmm_wire::Range {
            min: 400e6,
            max: 401e6,
            step: None,
        },
    ];
    let mut b = radio(B, tuned(400e6));
    b.settings.sample_rate = Some(96_000.0);
    let mut upper = decoder("upper", 100_021_000.0, &[A, B]);
    upper.settings.params = ChannelParams::Ssb(sdrmm_wire::SsbParams {
        sideband: sdrmm_wire::Sideband::Usb,
        bandwidth_hz: 3_000.0,
    });
    let mut lower = decoder("lower", 100_021_000.0, &[A, B]);
    lower.settings.params = ChannelParams::Ssb(sdrmm_wire::SsbParams {
        sideband: sdrmm_wire::Sideband::Lsb,
        bandwidth_hz: 3_000.0,
    });
    let mut wide = decoder("wide", 400e6, &[A, B]);
    wide.settings.params = ChannelSettings::default_for("wfm").unwrap().params;
    verify_exact(&[upper, lower, wide, decoder("gap", 250e6, &[A])], &[a, b]);
}

#[test]
fn touching_band_edges_and_duplicate_lanes_are_counted_once() {
    let mut a = radio(A, tuned(100e6));
    a.settings.sample_rate = Some(25_000.0);
    let decoders = [
        decoder("left", 100e6, &[A, A]),
        decoder("right", 100_012_500.0, &[A]),
    ];
    verify_exact(&decoders, &[a]);
}

#[test]
fn joint_window_search_improves_the_incumbent_and_proves_when_it_is_best() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [
        decoder("shared1", 100e6, &[A, B]),
        decoder("shared2", 100e6, &[A, B]),
        decoder("shared3", 100e6, &[A, B]),
        decoder("only_a1", 400e6, &[A]),
        decoder("only_a2", 400e6, &[A]),
        decoder("only_b", 700e6, &[B]),
    ];
    let partial = allocate(
        &decoders,
        &radios,
        Limits {
            duration: Duration::from_secs(10),
            nodes: 1,
        },
    );
    assert_eq!(partial.coverage.heard, 4);
    assert!(!partial.coverage.optimal());
    assert!(partial.coverage.upper_bound >= 5);
    let complete = allocate(&decoders, &radios, unlimited());
    assert_eq!(complete.coverage.heard, 5);
    assert!(complete.coverage.optimal());
    assert_eq!(received(&decoders, &radios, &complete.placements), 5);
    assert_eq!(exhaustive(&decoders, &radios), 5);
}
