use std::collections::HashSet;

use super::{Lane, Placeable, Radio, coverage::Coverage, search::Budget};
use crate::{center_of, planning::tuner_reaches, sample_rate_of};

pub(super) struct Domain {
    pub radio: usize,
    pub stream: u32,
    pub options: Vec<Coverage>,
}

pub(super) struct Model {
    pub lanes: Vec<Vec<Lane>>,
    pub domains: Vec<Domain>,
    pub total: usize,
}

struct Band {
    bit: usize,
    frequency: f64,
    low: f64,
    high: f64,
}

impl Band {
    fn new(bit: usize, settings: &sdrmm_wire::ChannelSettings) -> Self {
        let (low, high) = sdrmm_channels::occupied_band(&settings.params);
        Self {
            bit,
            frequency: settings.frequency_hz,
            low,
            high,
        }
    }

    fn hears(&self, center: f64, rate: f64) -> bool {
        crate::runtime::reaches(self.frequency - center, self.low, self.high, rate)
    }

    fn span(&self, rate: f64) -> Option<(f64, f64)> {
        crate::planning::tuning_span(self.frequency, self.low, self.high, rate)
    }
}

pub(super) fn usable(decoder: &Placeable, radios: &[Radio]) -> Vec<Lane> {
    let mut lanes = Vec::new();
    for &lane in &decoder.lanes {
        if !lanes.contains(&lane)
            && radios
                .iter()
                .any(|radio| radio.device_set == lane.device_set && radio.has_stream(lane.stream))
        {
            lanes.push(lane);
        }
    }
    if let Some(held) = decoder.held
        && decoder.pinned
        && lanes.contains(&held)
    {
        return vec![held];
    }
    lanes
}

fn stream_of(radio: &Radio, lane: Lane) -> u32 {
    if radio.capabilities.per_stream.tuning {
        lane.stream
    } else {
        0
    }
}

fn options(
    radio: &Radio,
    stream: u32,
    bands: &[Band],
    total: usize,
    budget: &Budget,
) -> Option<Vec<Coverage>> {
    let current = center_of(&radio.settings, stream, &radio.capabilities.per_stream);
    let rate = sample_rate_of(&radio.settings);
    let follows = radio.has_stream(stream) && radio.follows(stream);
    let mut centers = vec![current];
    if follows {
        for band in bands {
            if budget.expired() {
                return None;
            }
            if let Some((low, high)) = band.span(rate) {
                centers.extend([low, high, low.next_up(), high.next_down()]);
            }
        }
        centers.extend(
            radio
                .capabilities
                .freq_ranges
                .iter()
                .flat_map(|range| [range.min, range.max]),
        );
        centers.retain(|hz| hz.is_finite() && tuner_reaches(&radio.capabilities, *hz));
        centers.sort_by(f64::total_cmp);
        centers.dedup();
        let midpoints: Vec<_> = centers
            .windows(2)
            .map(|pair| f64::midpoint(pair[0], pair[1]))
            .filter(|hz| tuner_reaches(&radio.capabilities, *hz))
            .collect();
        centers.extend(midpoints);
    }
    let mut unique = HashSet::new();
    let mut options = Vec::new();
    for center in centers {
        if budget.expired() {
            return None;
        }
        let mut covered = Coverage::empty(total);
        for band in bands {
            if band.hears(center, rate) {
                covered.insert(band.bit);
            }
        }
        if unique.insert(covered.clone()) {
            options.push(covered);
        }
    }
    options.sort_by_key(|option| std::cmp::Reverse(option.count()));
    let mut maximal: Vec<Coverage> = Vec::new();
    for option in options {
        if budget.expired() {
            return None;
        }
        if !maximal.iter().any(|other| option.subset_of(other)) {
            maximal.push(option);
        }
    }
    if maximal.is_empty() {
        maximal.push(Coverage::empty(total));
    }
    Some(maximal)
}

impl Model {
    pub(super) fn new(decoders: &[Placeable], radios: &[Radio], budget: &Budget) -> Option<Self> {
        let lanes: Vec<_> = decoders
            .iter()
            .map(|decoder| usable(decoder, radios))
            .collect();
        let total = decoders.len() + radios.iter().map(|radio| radio.fixed.len()).sum::<usize>();
        let mut domains = Vec::new();
        let mut fixed_bit = decoders.len();
        for (index, radio) in radios.iter().enumerate() {
            let mut streams: Vec<_> = lanes
                .iter()
                .flatten()
                .filter(|lane| lane.device_set == radio.device_set)
                .map(|&lane| stream_of(radio, lane))
                .collect();
            streams.extend(radio.fixed.iter().map(|channel| {
                stream_of(
                    radio,
                    Lane {
                        device_set: radio.device_set,
                        stream: channel.stream,
                    },
                )
            }));
            streams.sort_unstable();
            streams.dedup();
            for stream in streams {
                if budget.expired() {
                    return None;
                }
                let mut bands: Vec<_> = decoders
                    .iter()
                    .enumerate()
                    .filter(|(bit, _)| {
                        lanes[*bit].iter().any(|lane| {
                            lane.device_set == radio.device_set && stream_of(radio, *lane) == stream
                        })
                    })
                    .map(|(bit, decoder)| Band::new(bit, &decoder.settings))
                    .collect();
                bands.extend(
                    radio
                        .fixed
                        .iter()
                        .enumerate()
                        .filter(|(_, channel)| {
                            !radio.capabilities.per_stream.tuning || channel.stream == stream
                        })
                        .map(|(bit, channel)| Band::new(fixed_bit + bit, &channel.settings)),
                );
                domains.push(Domain {
                    radio: index,
                    stream,
                    options: options(radio, stream, &bands, total, budget)?,
                });
            }
            fixed_bit += radio.fixed.len();
        }
        domains.sort_by_key(|domain| {
            (
                domain.options.len(),
                radios[domain.radio].device_set,
                domain.stream,
            )
        });
        Some(Self {
            lanes,
            domains,
            total,
        })
    }

    pub(super) fn covers(
        &self,
        radios: &[Radio],
        chosen: &[usize],
        bit: usize,
        lane: Lane,
    ) -> bool {
        self.domains.iter().zip(chosen).any(|(domain, &option)| {
            let radio = &radios[domain.radio];
            radio.device_set == lane.device_set
                && stream_of(radio, lane) == domain.stream
                && domain.options[option].contains(bit)
        })
    }

    pub(super) fn greedy(&self, decoders: &[Placeable]) -> Vec<usize> {
        let mut covered = Coverage::empty(self.total);
        self.domains
            .iter()
            .map(|domain| {
                let chosen = domain
                    .options
                    .iter()
                    .enumerate()
                    .max_by_key(|(index, option)| {
                        let constrained = (0..self.total)
                            .filter(|&bit| {
                                option.contains(bit)
                                    && !covered.contains(bit)
                                    && (bit >= decoders.len() || self.lanes[bit].len() == 1)
                            })
                            .count();
                        (
                            option.gain(&covered),
                            constrained,
                            std::cmp::Reverse(*index),
                        )
                    })
                    .map_or(0, |(index, _)| index);
                covered = covered.union(&domain.options[chosen]);
                chosen
            })
            .collect()
    }
}
