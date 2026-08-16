use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::decode::DecoderEvent;

pub const MAX_FILTER_KINDS: usize = 64;
pub const MAX_FILTER_IDS: usize = 256;
pub const MAX_FILTER_DURATION_MS: u32 = 600_000;
pub const MAX_FILTER_TEXT_LEN: usize = 128;

pub const VOICE_KINDS: &[&str] = &["call", "dv"];

pub const POSITION_KINDS: &[&str] = &[
    "adsb",
    "ais",
    "aprs",
    "dv",
    "dsc",
    "inmarsat_stdc",
    "inmarsat_aero",
    "vdl2",
    "hfdl",
    "iridium",
];

pub const DURATION_KINDS: &[&str] = &["call"];

#[must_use]
pub fn predicates_for(kinds: &[String]) -> Vec<&'static str> {
    let touches = |applies: &[&str]| {
        kinds.is_empty() || kinds.iter().any(|kind| applies.contains(&kind.as_str()))
    };
    let mut shown = vec!["stations", "contains"];
    if touches(POSITION_KINDS) {
        shown.push("has_position");
    }
    if touches(VOICE_KINDS) {
        shown.extend(["talkgroups", "radios", "encrypted", "emergency"]);
    }
    if touches(DURATION_KINDS) {
        shown.push("min_duration_ms");
    }
    shown
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventFilterNode {
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub stations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_position: Option<bool>,
    #[serde(default)]
    pub talkgroups: Vec<u32>,
    #[serde(default)]
    pub radios: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emergency: Option<bool>,
    #[serde(default)]
    pub min_duration_ms: u32,
}

struct Voice {
    source: Option<u32>,
    destination: Option<u32>,
    encrypted: Option<bool>,
    emergency: Option<bool>,
    duration_ms: Option<u64>,
}

impl EventFilterNode {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.kinds.len() <= MAX_FILTER_KINDS
            && self.stations.len() <= MAX_FILTER_IDS
            && self.talkgroups.len() <= MAX_FILTER_IDS
            && self.radios.len() <= MAX_FILTER_IDS
            && self.min_duration_ms <= MAX_FILTER_DURATION_MS
            && self
                .contains
                .as_ref()
                .is_none_or(|text| text.len() <= MAX_FILTER_TEXT_LEN)
            && self
                .kinds
                .iter()
                .all(|kind| !kind.is_empty() && kind.len() <= 32)
            && self
                .stations
                .iter()
                .all(|station| !station.is_empty() && station.len() <= MAX_FILTER_TEXT_LEN)
    }

    #[must_use]
    pub fn passes(&self, event: &DecoderEvent) -> bool {
        if !self.kinds.is_empty() && !self.kinds.iter().any(|kind| kind == event.kind()) {
            return false;
        }
        if !self.stations.is_empty() {
            let Some(station) = event.station() else {
                return false;
            };
            if !self
                .stations
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(&station))
            {
                return false;
            }
        }
        if let Some(text) = &self.contains
            && !text.is_empty()
            && !event
                .summary()
                .to_lowercase()
                .contains(&text.to_lowercase())
        {
            return false;
        }
        if let Some(want) = self.has_position
            && want != event.position().is_some()
        {
            return false;
        }
        let Some(voice) = voice_of(event) else {
            return true;
        };
        if !self.talkgroups.is_empty()
            && !voice
                .destination
                .is_some_and(|id| self.talkgroups.contains(&id))
        {
            return false;
        }
        if !self.radios.is_empty() && !voice.source.is_some_and(|id| self.radios.contains(&id)) {
            return false;
        }
        if self
            .encrypted
            .is_some_and(|want| voice.encrypted != Some(want))
        {
            return false;
        }
        if self
            .emergency
            .is_some_and(|want| voice.emergency != Some(want))
        {
            return false;
        }
        voice
            .duration_ms
            .is_none_or(|held| held >= u64::from(self.min_duration_ms))
    }
}

fn voice_of(event: &DecoderEvent) -> Option<Voice> {
    match event {
        DecoderEvent::Call(c) => Some(Voice {
            source: c.source,
            destination: c.destination,
            encrypted: Some(c.encrypted),
            emergency: Some(c.emergency),
            duration_ms: Some(c.duration_ms),
        }),
        DecoderEvent::Dv(f) => Some(Voice {
            source: f.source,
            destination: f.destination,
            encrypted: f.encrypted,
            emergency: f.emergency,
            duration_ms: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdsbMessage, DvFrame, DvFrameKind, DvMode, RttyText, rest::VoiceCall};

    fn call() -> VoiceCall {
        VoiceCall {
            id: 1,
            node: "dmr".to_owned(),
            source_node: "dmr".to_owned(),
            started_at: "2026-08-16T10:00:00Z".to_owned(),
            ended_at: "2026-08-16T10:00:02Z".to_owned(),
            duration_ms: 2_000,
            device_set: 1,
            channel: 2,
            freq_hz: 451_125_000.0,
            mode: DvMode::Dmr,
            slot: Some(2),
            color_code: Some(3),
            source: Some(2_621_001),
            destination: Some(505),
            group_call: Some(true),
            encrypted: false,
            emergency: false,
            audio: None,
            audio_error: None,
        }
    }

    fn adsb(icao: &str, callsign: Option<&str>, position: bool) -> DecoderEvent {
        DecoderEvent::Adsb(AdsbMessage {
            icao: icao.to_owned(),
            df: 17,
            callsign: callsign.map(str::to_owned),
            lat: position.then_some(50.4),
            lon: position.then_some(6.6),
            ..AdsbMessage::default()
        })
    }

    fn rtty() -> DecoderEvent {
        DecoderEvent::Rtty(RttyText {
            text: "CQ TEST".to_owned(),
        })
    }

    fn voice_frame() -> DecoderEvent {
        DecoderEvent::Dv(DvFrame {
            source: Some(2_621_001),
            destination: Some(505),
            encrypted: Some(false),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Voice)
        })
    }

    #[test]
    fn an_empty_filter_passes_everything() {
        let filter = EventFilterNode::default();
        assert!(filter.passes(&rtty()));
        assert!(filter.passes(&DecoderEvent::Call(call())));
        assert!(filter.passes(&adsb("3C6444", None, true)));
    }

    #[test]
    fn naming_kinds_admits_only_those_kinds() {
        let filter = EventFilterNode {
            kinds: vec!["call".to_owned()],
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&DecoderEvent::Call(call())));
        assert!(!filter.passes(&rtty()));
        assert!(!filter.passes(&voice_frame()));
    }

    #[test]
    fn a_station_list_works_for_any_kind_that_names_one() {
        let filter = EventFilterNode {
            stations: vec!["3C6444".to_owned()],
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&adsb("3C6444", None, true)));
        assert!(!filter.passes(&adsb("4CA2D4", None, true)));
        assert!(
            !filter.passes(&rtty()),
            "an event with no station cannot be one of the named ones"
        );
        assert!(
            !filter.passes(&DecoderEvent::Call(call())),
            "a call names its radio, which is not in the list"
        );
    }

    #[test]
    fn a_station_list_ignores_case() {
        let filter = EventFilterNode {
            stations: vec!["3c6444".to_owned()],
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&adsb("3C6444", None, true)));
    }

    #[test]
    fn contains_searches_the_summary_of_any_kind() {
        let filter = EventFilterNode {
            contains: Some("baw".to_owned()),
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&adsb("3C6444", Some("BAW890"), true)));
        assert!(!filter.passes(&adsb("3C6444", Some("RYR9AB"), true)));
    }

    #[test]
    fn a_position_predicate_keeps_only_the_fixes() {
        let wants = EventFilterNode {
            has_position: Some(true),
            ..EventFilterNode::default()
        };
        assert!(wants.passes(&adsb("3C6444", None, true)));
        assert!(!wants.passes(&adsb("3C6444", None, false)));
        assert!(!wants.passes(&rtty()));

        let without = EventFilterNode {
            has_position: Some(false),
            ..EventFilterNode::default()
        };
        assert!(without.passes(&rtty()));
        assert!(!without.passes(&adsb("3C6444", None, true)));
    }

    #[test]
    fn the_voice_predicates_reach_raw_frames_as_well_as_calls() {
        let filter = EventFilterNode {
            talkgroups: vec![505],
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&DecoderEvent::Call(call())));
        assert!(filter.passes(&voice_frame()));
        assert!(!filter.passes(&DecoderEvent::Call(VoiceCall {
            destination: Some(77),
            ..call()
        })));
    }

    #[test]
    fn a_radio_list_admits_only_those_radios() {
        let filter = EventFilterNode {
            radios: vec![2_621_001],
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&DecoderEvent::Call(call())));
        assert!(!filter.passes(&DecoderEvent::Call(VoiceCall {
            source: Some(9),
            ..call()
        })));
    }

    #[test]
    fn the_encryption_and_emergency_flags_are_three_state() {
        let clear = EventFilterNode {
            encrypted: Some(false),
            ..EventFilterNode::default()
        };
        assert!(clear.passes(&DecoderEvent::Call(call())));
        assert!(!clear.passes(&DecoderEvent::Call(VoiceCall {
            encrypted: true,
            ..call()
        })));

        let urgent = EventFilterNode {
            emergency: Some(true),
            ..EventFilterNode::default()
        };
        assert!(!urgent.passes(&DecoderEvent::Call(call())));
        assert!(urgent.passes(&DecoderEvent::Call(VoiceCall {
            emergency: true,
            ..call()
        })));
    }

    #[test]
    fn a_minimum_duration_only_judges_what_has_one() {
        let filter = EventFilterNode {
            min_duration_ms: 1_500,
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&DecoderEvent::Call(call())));
        assert!(!filter.passes(&DecoderEvent::Call(VoiceCall {
            duration_ms: 400,
            ..call()
        })));
        assert!(
            filter.passes(&voice_frame()),
            "a raw frame has no duration to judge"
        );
    }

    #[test]
    fn voice_predicates_leave_other_kinds_alone() {
        let filter = EventFilterNode {
            talkgroups: vec![505],
            radios: vec![1],
            encrypted: Some(true),
            emergency: Some(true),
            min_duration_ms: 60_000,
            ..EventFilterNode::default()
        };
        assert!(
            filter.passes(&adsb("3C6444", None, true)),
            "a voice predicate must not silently drop unrelated kinds"
        );
    }

    #[test]
    fn every_predicate_has_to_agree() {
        let filter = EventFilterNode {
            kinds: vec!["call".to_owned()],
            talkgroups: vec![505],
            min_duration_ms: 1_000,
            ..EventFilterNode::default()
        };
        assert!(filter.passes(&DecoderEvent::Call(call())));
        assert!(!filter.passes(&DecoderEvent::Call(VoiceCall {
            duration_ms: 100,
            ..call()
        })));
    }

    #[test]
    fn only_the_predicates_that_suit_the_wired_kinds_are_offered() {
        assert_eq!(
            predicates_for(&["adsb".to_owned()]),
            vec!["stations", "contains", "has_position"],
            "an aircraft has no talkgroup"
        );
        assert_eq!(
            predicates_for(&["pocsag".to_owned()]),
            vec!["stations", "contains"],
            "a pager has neither talkgroup nor position"
        );
        assert_eq!(
            predicates_for(&["call".to_owned()]),
            vec![
                "stations",
                "contains",
                "talkgroups",
                "radios",
                "encrypted",
                "emergency",
                "min_duration_ms"
            ]
        );
        assert!(
            predicates_for(&["adsb".to_owned(), "call".to_owned()]).contains(&"talkgroups"),
            "a mixed wire offers the union"
        );
        assert!(
            predicates_for(&[]).contains(&"talkgroups"),
            "with nothing wired we cannot narrow, so offer everything"
        );
    }

    #[test]
    fn oversized_lists_are_refused() {
        assert!(EventFilterNode::default().valid());
        assert!(
            !EventFilterNode {
                talkgroups: vec![0; MAX_FILTER_IDS + 1],
                ..EventFilterNode::default()
            }
            .valid()
        );
        assert!(
            !EventFilterNode {
                kinds: vec![String::new()],
                ..EventFilterNode::default()
            }
            .valid()
        );
        assert!(
            !EventFilterNode {
                contains: Some("x".repeat(MAX_FILTER_TEXT_LEN + 1)),
                ..EventFilterNode::default()
            }
            .valid()
        );
        assert!(
            !EventFilterNode {
                min_duration_ms: MAX_FILTER_DURATION_MS + 1,
                ..EventFilterNode::default()
            }
            .valid()
        );
    }
}
