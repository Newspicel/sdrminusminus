use serde_json::json;

use super::*;

fn kinds(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn descriptor(type_id: &str, decoder_kind: Option<&str>) -> ChannelDescriptor {
    serde_json::from_value(json!({
        "type_id": type_id, "name": type_id, "bandwidth_hz": 12_500.0,
        "input_rate_hz": 48_000.0, "decoder_kind": decoder_kind,
    }))
    .unwrap_or_else(|error| panic!("descriptor: {error}"))
}

fn descriptors() -> Vec<ChannelDescriptor> {
    vec![
        descriptor("adsb", Some("adsb")),
        descriptor("dmr", Some("dv")),
        descriptor("pocsag", Some("pocsag")),
        descriptor("am", None),
    ]
}

fn channel(channel_type: &str, records_calls: bool) -> WiredSource {
    WiredSource {
        channel_type: Some(channel_type.to_owned()),
        records_calls,
        ..WiredSource::default()
    }
}

#[test]
fn ids_split_on_commas_spaces_and_newlines() {
    assert_eq!(parse_ids("505, 9\n77  1"), [505, 9, 77, 1]);
    assert_eq!(parse_ids("505, abc, -3, 1.5, , 9"), [505, 9]);
    assert_eq!(parse_ids("505 505 505"), [505]);
    let many = (0..MAX_FILTER_IDS + 50)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(parse_ids(&many).len(), MAX_FILTER_IDS);
    assert_eq!(parse_ids(&format_ids(&[505, 9])), [505, 9]);
}

#[test]
fn a_tri_state_round_trips() {
    assert_eq!(TriState::of(None), TriState::Any);
    assert_eq!(TriState::of(Some(true)), TriState::Yes);
    assert_eq!(TriState::of(Some(false)), TriState::No);
    assert_eq!(TriState::Any.value(), None);
    assert_eq!(TriState::Yes.value(), Some(true));
    assert_eq!(TriState::No.value(), Some(false));
}

#[test]
fn only_what_wired_decoders_emit_is_offered() {
    let table = descriptors();
    assert_eq!(kinds_offered(&[channel("adsb", false)], &table), ["adsb"]);
    assert_eq!(kinds_offered(&[channel("dmr", false)], &table), ["dv"]);
    assert_eq!(
        kinds_offered(&[channel("dmr", true)], &table),
        ["call", "dv"]
    );
    let trunk = WiredSource {
        records_calls: true,
        trunk: true,
        ..WiredSource::default()
    };
    assert_eq!(kinds_offered(&[trunk], &table), ["call", "dv"]);
    assert!(kinds_offered(&[channel("am", false)], &table).is_empty());
    let many = [
        channel("adsb", false),
        channel("adsb", false),
        channel("pocsag", false),
    ];
    assert_eq!(kinds_offered(&many, &table), ["adsb", "pocsag"]);
}

#[test]
fn a_spectrum_monitor_offers_transmissions() {
    let monitor = WiredSource {
        monitor: true,
        ..WiredSource::default()
    };
    assert!(kinds_offered(&[monitor], &[]).contains(&"transmission".to_owned()));
}

#[test]
fn predicates_follow_the_facets_of_the_wire() {
    use Predicate::*;
    assert_eq!(
        predicates_for(&kinds(&["adsb"])),
        [Stations, Contains, HasPosition]
    );
    assert_eq!(predicates_for(&kinds(&["pocsag"])), [Stations, Contains]);
    assert_eq!(
        predicates_for(&kinds(&["call"])),
        [
            Stations,
            Contains,
            Talkgroups,
            Radios,
            Encrypted,
            Emergency,
            MinDuration
        ]
    );
    assert!(!predicates_for(&kinds(&["dv"])).contains(&MinDuration));
    assert!(predicates_for(&kinds(&["dv"])).contains(&Talkgroups));
    let mixed = predicates_for(&kinds(&["adsb", "call"]));
    assert!(mixed.contains(&HasPosition) && mixed.contains(&Talkgroups));
}

#[test]
fn the_station_field_names_what_the_wire_carries() {
    assert_eq!(station_label(&kinds(&["adsb"])), "Aircraft");
    assert_eq!(station_label(&kinds(&["ais"])), "Vessels");
    assert_eq!(station_label(&kinds(&["call", "dv"])), "Radios seen");
    assert_eq!(station_label(&kinds(&["adsb", "pocsag"])), "Stations");
    assert_eq!(station_label(&[]), "Stations");
}

#[test]
fn words_split_and_keep_each_once() {
    assert_eq!(parse_words("BAW890, RYR9AB  BAW890"), ["BAW890", "RYR9AB"]);
    assert!(parse_words(" , ,  ").is_empty());
}

#[test]
fn the_subtitle_says_what_the_filter_does() {
    assert_eq!(filter_said(&EventFilterNode::default()), "keep every event");
    let voice = EventFilterNode {
        kinds: kinds(&["call"]),
        talkgroups: vec![505],
        radios: vec![1001],
        encrypted: Some(false),
        emergency: Some(true),
        min_duration_ms: 1_500,
        ..EventFilterNode::default()
    };
    assert_eq!(
        filter_said(&voice),
        "keep call · TG 505 · radio 1001 · clear · emergency · over 1.5 s"
    );
    let drop = EventFilterNode {
        mode: FilterMode::Drop,
        kinds: kinds(&["pocsag"]),
        contains: Some("TEST".to_owned()),
        ..EventFilterNode::default()
    };
    assert_eq!(filter_said(&drop), "drop pocsag · \"TEST\"");
    let empty_drop = EventFilterNode {
        mode: FilterMode::Drop,
        ..EventFilterNode::default()
    };
    assert_eq!(filter_said(&empty_drop), "drop nothing");
}

fn titles(names: &[&str]) -> Vec<&'static str> {
    sections_for(&kinds(names))
        .iter()
        .map(|section| section.title)
        .collect()
}

#[test]
fn sections_show_only_what_applies() {
    assert_eq!(titles(&["adsb"]), ["Any event", "Position"]);
    assert_eq!(titles(&["pocsag"]), ["Any event"]);
    assert_eq!(titles(&["call"]), ["Any event", "Voice"]);
    assert_eq!(
        titles(&["adsb", "call"]),
        ["Any event", "Position", "Voice"]
    );
    let mixed: Vec<(&str, Vec<String>)> = sections_for(&kinds(&["adsb", "call"]))
        .into_iter()
        .map(|section| (section.title, section.applies))
        .collect();
    assert_eq!(
        mixed,
        [
            ("Any event", kinds(&["adsb", "call"])),
            ("Position", kinds(&["adsb"])),
            ("Voice", kinds(&["call"])),
        ]
    );
}
