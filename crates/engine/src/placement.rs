use std::collections::HashSet;

use sdrmm_wire::{Capabilities, ChannelInfo, ChannelSettings, DeviceSettings, PlacementCoverage};

use crate::{
    Engine,
    planning::{hears, plan_center},
};

mod coverage;
mod model;
mod search;

pub(crate) use search::Limits;

#[cfg(test)]
pub(crate) mod heuristic;

pub struct Allocation {
    pub placements: Vec<Placement>,
    pub coverage: PlacementCoverage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lane {
    pub device_set: u32,
    pub stream: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placeable {
    pub node: String,
    pub settings: ChannelSettings,
    pub lanes: Vec<Lane>,
    pub held: Option<Lane>,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub node: String,
    pub lane: Lane,
}

pub(crate) struct Radio {
    pub device_set: u32,
    pub capabilities: Capabilities,
    pub settings: DeviceSettings,
    pub tunes_freely: bool,
    pub group: Vec<u32>,
    pub fixed: Vec<ChannelInfo>,
}

impl Radio {
    fn has_stream(&self, stream: u32) -> bool {
        stream < self.capabilities.rx_streams.max(1)
    }

    fn settled(&self, carried: &[ChannelInfo]) -> DeviceSettings {
        let mut settings = self.settings.clone();
        if self.tunes_freely
            && let Some(delta) = plan_center(&self.capabilities, &settings, carried, &self.group)
        {
            settings.merge_from(&delta);
        }
        settings
    }

    fn hears(&self, settings: &DeviceSettings, channel: &ChannelInfo) -> bool {
        hears(
            &self.capabilities,
            settings,
            channel.stream,
            &channel.settings,
        )
    }

    fn follows(&self, stream: u32) -> bool {
        self.tunes_freely
            && self
                .settings
                .for_stream(stream, &self.capabilities.per_stream)
                .tunes_itself()
    }
}

impl Engine {
    #[must_use]
    pub fn place_channels(&self, decoders: &[Placeable]) -> Allocation {
        let nodes: HashSet<&str> = decoders.iter().map(|d| d.node.as_str()).collect();
        let radios: Vec<Radio> = {
            let inner = self.lock();
            let arrayed: HashSet<u32> = inner
                .device_sets
                .values()
                .filter_map(|state| state.array.as_ref())
                .flat_map(|array| array.member_sets())
                .collect();
            inner
                .device_sets
                .iter()
                .map(|(id, state)| Radio {
                    device_set: *id,
                    capabilities: state.capabilities.clone(),
                    settings: state.settings.clone(),
                    tunes_freely: state.tunes_freely() && !arrayed.contains(id),
                    group: state.coherent_lanes(),
                    fixed: state
                        .channels
                        .iter()
                        .filter(|channel| {
                            channel
                                .node
                                .as_deref()
                                .is_none_or(|node| !nodes.contains(node))
                        })
                        .cloned()
                        .collect(),
                })
                .collect()
        };
        allocate(decoders, &radios, search::Limits::default())
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

fn fallback(decoder: &Placeable, lanes: &[Lane], radios: &[Radio]) -> Option<Lane> {
    lanes
        .iter()
        .copied()
        .enumerate()
        .max_by_key(|(index, lane)| {
            let radio = radios
                .iter()
                .find(|radio| radio.device_set == lane.device_set);
            let heard = radio.is_some_and(|radio| {
                radio.hears(&radio.settings, &carried_info(decoder, lane.stream))
            });
            let reaches = radio.is_some_and(|radio| {
                crate::planning::tuner_reaches(&radio.capabilities, decoder.settings.frequency_hz)
            });
            (
                heard,
                reaches,
                decoder.held == Some(*lane),
                std::cmp::Reverse(*index),
            )
        })
        .map(|(_, lane)| lane)
}

pub(crate) fn received(
    decoders: &[Placeable],
    radios: &[Radio],
    placements: &[Placement],
) -> usize {
    radios
        .iter()
        .map(|radio| {
            let mut carried = radio.fixed.clone();
            for decoder in decoders {
                if let Some(placement) = placements.iter().find(|placement| {
                    placement.node == decoder.node && placement.lane.device_set == radio.device_set
                }) {
                    carried.push(carried_info(decoder, placement.lane.stream));
                }
            }
            let tuning = radio.settled(&carried);
            carried
                .iter()
                .filter(|channel| radio.hears(&tuning, channel))
                .count()
        })
        .sum()
}

fn project(
    decoders: &[Placeable],
    radios: &[Radio],
    model: &model::Model,
    chosen: &[usize],
) -> Vec<Placement> {
    decoders
        .iter()
        .enumerate()
        .filter_map(|(bit, decoder)| {
            let covered: Vec<_> = model.lanes[bit]
                .iter()
                .copied()
                .filter(|&lane| model.covers(radios, chosen, bit, lane))
                .collect();
            let lanes = if covered.is_empty() {
                &model.lanes[bit]
            } else {
                &covered
            };
            let lane = fallback(decoder, lanes, radios)?;
            Some(Placement {
                node: decoder.node.clone(),
                lane,
            })
        })
        .collect()
}

pub(crate) fn allocate(decoders: &[Placeable], radios: &[Radio], limits: Limits) -> Allocation {
    let budget = search::Budget::new(limits);
    let mut placements: Vec<_> = decoders
        .iter()
        .filter_map(|decoder| {
            let lane = fallback(decoder, &model::usable(decoder, radios), radios)?;
            Some(Placement {
                node: decoder.node.clone(),
                lane,
            })
        })
        .collect();
    let mut heard = received(decoders, radios, &placements);
    let total = decoders.len() + radios.iter().map(|radio| radio.fixed.len()).sum::<usize>();
    let coverage = |heard: usize, upper: usize| PlacementCoverage {
        heard: heard as u32,
        upper_bound: upper.max(heard) as u32,
    };
    if heard == total {
        return Allocation {
            placements,
            coverage: coverage(heard, heard),
        };
    }
    let Some(model) = model::Model::new(decoders, radios, &budget) else {
        return Allocation {
            placements,
            coverage: coverage(heard, total),
        };
    };
    let greedy = project(decoders, radios, &model, &model.greedy(decoders));
    let greedy_heard = received(decoders, radios, &greedy);
    if greedy_heard > heard {
        placements = greedy;
        heard = greedy_heard;
    }
    let mut search = search::Search::new(&model.domains, total, heard, budget);
    search.run(total);
    if let Some(chosen) = &search.chosen {
        let candidate = project(decoders, radios, &model, chosen);
        let candidate_heard = received(decoders, radios, &candidate);
        if candidate_heard > heard {
            placements = candidate;
            heard = candidate_heard;
        }
    }
    let upper_bound = if heard > search.upper_bound {
        total
    } else {
        search.upper_bound
    };
    Allocation {
        placements,
        coverage: coverage(heard, upper_bound),
    }
}

#[cfg(test)]
pub(crate) fn place(decoders: &[Placeable], radios: &[Radio]) -> Vec<Placement> {
    allocate(decoders, radios, search::Limits::default()).placements
}
