use std::collections::BTreeSet;

use sdrmm_wire::{
    channel::ChannelDescriptor,
    filter::{
        EventFacet, EventFilterNode, FilterMode, MAX_FILTER_IDS, MAX_FILTER_TEXT_LEN, facets_of,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriState {
    Any,
    Yes,
    No,
}

impl TriState {
    #[must_use]
    pub fn of(value: Option<bool>) -> Self {
        match value {
            None => Self::Any,
            Some(true) => Self::Yes,
            Some(false) => Self::No,
        }
    }

    #[must_use]
    pub fn value(self) -> Option<bool> {
        match self {
            Self::Any => None,
            Self::Yes => Some(true),
            Self::No => Some(false),
        }
    }
}

fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|token| !token.is_empty())
}

#[must_use]
pub fn parse_ids(text: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    for id in tokens(text).filter_map(|token| token.parse::<u32>().ok()) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.truncate(MAX_FILTER_IDS);
    ids
}

#[must_use]
pub fn format_ids(ids: &[u32]) -> String {
    ids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[must_use]
pub fn parse_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in tokens(text).filter(|word| word.len() <= MAX_FILTER_TEXT_LEN) {
        if !words.iter().any(|held| held == word) {
            words.push(word.to_owned());
        }
    }
    words.truncate(MAX_FILTER_IDS);
    words
}

fn every_kind_has(kinds: &[String], facet: EventFacet) -> bool {
    !kinds.is_empty() && kinds.iter().all(|kind| facets_of(kind).contains(&facet))
}

#[must_use]
pub fn station_label(kinds: &[String]) -> &'static str {
    let only = |named: &str| !kinds.is_empty() && kinds.iter().all(|kind| kind == named);
    if only("adsb") {
        "Aircraft"
    } else if only("ais") {
        "Vessels"
    } else if every_kind_has(kinds, EventFacet::Voice) {
        "Radios seen"
    } else {
        "Stations"
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WiredSource {
    pub channel_type: Option<String>,
    pub records_calls: bool,
    pub trunk: bool,
    pub monitor: bool,
}

#[must_use]
pub fn kinds_offered(sources: &[WiredSource], descriptors: &[ChannelDescriptor]) -> Vec<String> {
    let mut kinds = BTreeSet::new();
    for source in sources {
        if source.monitor {
            kinds.insert("transmission".to_owned());
            kinds.extend(descriptors.iter().filter_map(|d| d.decoder_kind.clone()));
            kinds.insert("broadcast_data".to_owned());
        }
        if source.trunk {
            kinds.insert("dv".to_owned());
        }
        let decoded = descriptors
            .iter()
            .find(|d| Some(&d.type_id) == source.channel_type.as_ref())
            .and_then(|d| d.decoder_kind.clone());
        if let Some(kind) = decoded {
            if kind == "broadcast" {
                kinds.insert("broadcast_data".to_owned());
            }
            kinds.insert(kind);
        }
        if source.records_calls {
            kinds.insert("call".to_owned());
        }
    }
    kinds.into_iter().collect()
}

#[must_use]
pub fn filter_said(filter: &EventFilterNode) -> String {
    let mut parts = vec![if filter.kinds.is_empty() {
        "every event".to_owned()
    } else {
        filter.kinds.join(", ")
    }];
    if !filter.stations.is_empty() {
        parts.push(filter.stations.join(", "));
    }
    if let Some(text) = filter.contains.as_ref().filter(|text| !text.is_empty()) {
        parts.push(format!("\"{text}\""));
    }
    if let Some(fix) = filter.has_position {
        parts.push(if fix { "with a fix" } else { "without a fix" }.to_owned());
    }
    if !filter.talkgroups.is_empty() {
        parts.push(format!("TG {}", format_ids(&filter.talkgroups)));
    }
    if !filter.radios.is_empty() {
        parts.push(format!("radio {}", format_ids(&filter.radios)));
    }
    if let Some(encrypted) = filter.encrypted {
        parts.push(if encrypted { "encrypted" } else { "clear" }.to_owned());
    }
    if let Some(emergency) = filter.emergency {
        parts.push(if emergency { "emergency" } else { "routine" }.to_owned());
    }
    if filter.min_duration_ms > 0 {
        parts.push(format!(
            "over {:.1} s",
            f64::from(filter.min_duration_ms) / 1000.0
        ));
    }
    let mode = match filter.mode {
        FilterMode::Keep => "keep",
        FilterMode::Drop => "drop",
    };
    if filter.mode == FilterMode::Drop && parts.len() == 1 && filter.kinds.is_empty() {
        return "drop nothing".to_owned();
    }
    format!("{mode} {}", parts.join(" · "))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predicate {
    Stations,
    Contains,
    HasPosition,
    Talkgroups,
    Radios,
    Encrypted,
    Emergency,
    MinDuration,
}

#[must_use]
pub fn predicates_for(kinds: &[String]) -> Vec<Predicate> {
    let touches =
        |facet| kinds.is_empty() || kinds.iter().any(|kind| facets_of(kind).contains(&facet));
    let mut shown = vec![Predicate::Stations, Predicate::Contains];
    if touches(EventFacet::Position) {
        shown.push(Predicate::HasPosition);
    }
    if touches(EventFacet::Voice) {
        shown.extend([
            Predicate::Talkgroups,
            Predicate::Radios,
            Predicate::Encrypted,
            Predicate::Emergency,
        ]);
    }
    if touches(EventFacet::Duration) {
        shown.push(Predicate::MinDuration);
    }
    shown
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub applies: Vec<String>,
    pub predicates: Vec<Predicate>,
}

#[must_use]
pub fn sections_for(kinds: &[String]) -> Vec<Section> {
    let shown = predicates_for(kinds);
    let pick = |keys: &[Predicate]| -> Vec<Predicate> {
        keys.iter()
            .copied()
            .filter(|key| shown.contains(key))
            .collect()
    };
    let scope = |facet: EventFacet| -> Vec<String> {
        let mut applies: Vec<String> = kinds
            .iter()
            .filter(|kind| facets_of(kind).contains(&facet))
            .cloned()
            .collect();
        applies.sort();
        applies
    };
    let mut every = kinds.to_vec();
    every.sort();
    let candidates = [
        (
            "Any event",
            every,
            pick(&[Predicate::Stations, Predicate::Contains]),
        ),
        (
            "Position",
            scope(EventFacet::Position),
            pick(&[Predicate::HasPosition]),
        ),
        (
            "Voice",
            scope(EventFacet::Voice),
            pick(&[
                Predicate::Talkgroups,
                Predicate::Radios,
                Predicate::Encrypted,
                Predicate::Emergency,
                Predicate::MinDuration,
            ]),
        ),
    ];
    candidates
        .into_iter()
        .filter(|(_, _, predicates)| !predicates.is_empty())
        .map(|(title, applies, predicates)| Section {
            title,
            applies,
            predicates,
        })
        .collect()
}

#[cfg(test)]
mod tests;
