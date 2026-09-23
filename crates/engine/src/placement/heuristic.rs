use std::collections::HashMap;

use super::*;
use crate::{center_of, planning::tuner_reaches};

const IMPROVEMENT_ROUNDS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Fit {
    heard: bool,
    kept: i64,
    reaches: bool,
    stays: bool,
    still: bool,
    load: i64,
    order: i64,
}

#[derive(Clone)]
struct Bench<'a> {
    radios: &'a [Radio],
    prefer_held: bool,
    carried: HashMap<u32, Vec<ChannelInfo>>,
    settled: HashMap<u32, DeviceSettings>,
}

impl<'a> Bench<'a> {
    fn new(radios: &'a [Radio]) -> Self {
        let carried: HashMap<u32, Vec<ChannelInfo>> = radios
            .iter()
            .map(|radio| (radio.device_set, radio.fixed.clone()))
            .collect();
        let settled = radios
            .iter()
            .map(|radio| (radio.device_set, radio.settled(&radio.fixed)))
            .collect();
        Self {
            radios,
            prefer_held: true,
            carried,
            settled,
        }
    }

    fn radio(&self, device_set: u32) -> Option<&'a Radio> {
        self.radios
            .iter()
            .find(|radio| radio.device_set == device_set)
    }

    fn usable(&self, decoder: &Placeable) -> Vec<Lane> {
        let wired: Vec<Lane> = decoder
            .lanes
            .iter()
            .copied()
            .filter(|lane| {
                self.radio(lane.device_set)
                    .is_some_and(|r| r.has_stream(lane.stream))
            })
            .collect();
        match decoder.held {
            Some(held) if decoder.pinned && wired.contains(&held) => vec![held],
            _ => wired,
        }
    }

    fn reachable_count(&self, decoder: &Placeable) -> usize {
        self.usable(decoder)
            .into_iter()
            .filter(|lane| {
                self.radio(lane.device_set)
                    .is_some_and(|r| tuner_reaches(&r.capabilities, decoder.settings.frequency_hz))
            })
            .count()
    }

    fn fit(&self, decoder: &Placeable, lane: Lane, order: usize) -> Option<Fit> {
        let radio = self.radio(lane.device_set)?;
        let carried = self.carried.get(&lane.device_set)?;
        let before = self.settled.get(&lane.device_set)?;
        let mine = carried_info(decoder, lane.stream);
        let reaches = tuner_reaches(&radio.capabilities, decoder.settings.frequency_hz);
        let stays = self.prefer_held && decoder.held == Some(lane);
        let load = carried.len() as i64;
        let order = -(order as i64);
        if !radio.follows(lane.stream) {
            return Some(Fit {
                heard: radio.hears(before, &mine),
                kept: 0,
                stays,
                still: true,
                reaches,
                load,
                order,
            });
        }
        let mut trial = carried.clone();
        trial.push(mine.clone());
        let after = radio.settled(&trial);
        let lost = carried
            .iter()
            .filter(|channel| radio.hears(before, channel) && !radio.hears(&after, channel))
            .count();
        let scope = radio.capabilities.per_stream;
        Some(Fit {
            heard: radio.hears(&after, &mine),
            kept: -(lost as i64),
            stays,
            still: center_of(&after, lane.stream, &scope) == center_of(before, lane.stream, &scope),
            reaches,
            load,
            order,
        })
    }

    fn choose(&self, decoder: &Placeable) -> Option<(Lane, Fit)> {
        self.usable(decoder)
            .into_iter()
            .enumerate()
            .filter_map(|(order, lane)| self.fit(decoder, lane, order).map(|fit| (lane, fit)))
            .max_by_key(|(_, fit)| *fit)
    }

    fn carry(&mut self, decoder: &Placeable, lane: Lane) {
        if let Some(carried) = self.carried.get_mut(&lane.device_set) {
            carried.push(carried_info(decoder, lane.stream));
        }
        self.resettle(lane.device_set);
    }

    fn release(&mut self, decoder: &Placeable, lane: Lane) {
        if let Some(carried) = self.carried.get_mut(&lane.device_set) {
            carried.retain(|channel| channel.node.as_deref() != Some(decoder.node.as_str()));
        }
        self.resettle(lane.device_set);
    }

    fn resettle(&mut self, device_set: u32) {
        if let (Some(radio), Some(carried)) =
            (self.radio(device_set), self.carried.get(&device_set))
        {
            self.settled.insert(device_set, radio.settled(carried));
        }
    }

    fn heard_count(&self) -> usize {
        self.radios
            .iter()
            .map(|radio| {
                let Some(settings) = self.settled.get(&radio.device_set) else {
                    return 0;
                };
                self.carried.get(&radio.device_set).map_or(0, |channels| {
                    channels
                        .iter()
                        .filter(|channel| radio.hears(settings, channel))
                        .count()
                })
            })
            .sum()
    }
}

fn carried_info(decoder: &Placeable, stream: u32) -> ChannelInfo {
    ChannelInfo {
        id: 0,
        stream,
        node: Some(decoder.node.clone()),
        settings: decoder.settings.clone(),
        out_of_band: false,
        audio_recordings: Vec::new(),
        baseband_recording: None,
        network_export: None,
    }
}

fn order(decoders: &[Placeable], bench: &Bench<'_>) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..decoders.len()).collect();
    indices.sort_by_key(|&index| (bench.reachable_count(&decoders[index]), index));
    indices
}

fn improve(decoders: &[Placeable], bench: &mut Bench<'_>, placed: &mut [Placement]) {
    for _ in 0..IMPROVEMENT_ROUNDS {
        let mut moved = false;
        for placement in placed.iter_mut() {
            let Some(decoder) = decoders.iter().find(|d| d.node == placement.node) else {
                continue;
            };
            let mut best_count = bench.heard_count();
            let mut best = None;
            for lane in bench.usable(decoder) {
                if lane == placement.lane {
                    continue;
                }
                let mut trial = bench.clone();
                trial.release(decoder, placement.lane);
                trial.carry(decoder, lane);
                let heard = trial.heard_count();
                if heard > best_count {
                    best_count = heard;
                    best = Some((lane, trial));
                }
            }
            if let Some((lane, trial)) = best {
                *bench = trial;
                placement.lane = lane;
                moved = true;
            }
        }
        if !moved {
            return;
        }
    }
}

fn place_ordered(
    decoders: &[Placeable],
    bench: &mut Bench<'_>,
    indices: &[usize],
) -> Vec<Placement> {
    let mut placed = Vec::with_capacity(decoders.len());
    for &index in indices {
        let decoder = &decoders[index];
        let Some((lane, _)) = bench.choose(decoder) else {
            continue;
        };
        bench.carry(decoder, lane);
        placed.push(Placement {
            node: decoder.node.clone(),
            lane,
        });
    }
    improve(decoders, bench, &mut placed);
    placed
}

fn placement_score(
    decoders: &[Placeable],
    bench: &Bench<'_>,
    placed: &[Placement],
) -> (usize, usize) {
    let stayed = placed
        .iter()
        .filter(|placement| {
            decoders.iter().any(|decoder| {
                decoder.node == placement.node && decoder.held == Some(placement.lane)
            })
        })
        .count();
    (bench.heard_count(), stayed)
}

pub(crate) fn place(decoders: &[Placeable], radios: &[Radio]) -> Vec<Placement> {
    let mut bench = Bench::new(radios);
    let mut indices = order(decoders, &bench);
    let mut placed = place_ordered(decoders, &mut bench, &indices);
    let mut score = placement_score(decoders, &bench, &placed);
    let total = decoders.len() + radios.iter().map(|radio| radio.fixed.len()).sum::<usize>();
    for descending in [false, true] {
        if score.0 == total {
            break;
        }
        let mut trial = Bench::new(radios);
        trial.prefer_held = false;
        indices.sort_by(|&a, &b| {
            trial
                .reachable_count(&decoders[a])
                .cmp(&trial.reachable_count(&decoders[b]))
                .then_with(|| {
                    let frequency = decoders[a]
                        .settings
                        .frequency_hz
                        .total_cmp(&decoders[b].settings.frequency_hz);
                    if descending {
                        frequency.reverse()
                    } else {
                        frequency
                    }
                })
                .then(a.cmp(&b))
        });
        let candidate = place_ordered(decoders, &mut trial, &indices);
        let candidate_score = placement_score(decoders, &trial, &candidate);
        if candidate_score > score {
            placed = candidate;
            score = candidate_score;
        }
    }
    placed.sort_by_key(|placement| {
        decoders
            .iter()
            .position(|decoder| decoder.node == placement.node)
    });
    placed
}
