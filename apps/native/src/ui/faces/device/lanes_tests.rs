use serde_json::{Value, json};

use super::*;

fn caps(extra: Value) -> Capabilities {
    let mut base = json!({
        "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": []
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    serde_json::from_value(base).expect("capabilities")
}

fn set(extra: Value) -> DeviceSet {
    let mut base = json!({
        "id": 1,
        "device": { "driver": "virtual", "key": "siggen", "label": "Signal Generator" },
        "capabilities": caps(json!({})),
        "settings": {},
        "status": "running",
        "channels": [],
        "overruns": 0
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    serde_json::from_value(base).expect("device set")
}

fn channels(out: &[bool]) -> Value {
    Value::Array(
        out.iter()
            .enumerate()
            .map(|(id, out_of_band)| {
                json!({
                    "id": id, "stream": 0, "out_of_band": out_of_band,
                    "settings": { "frequency_hz": 100e6, "params": { "type": "nfm", "settings": {} } }
                })
            })
            .collect(),
    )
}

fn reference(backend: &str, serial: Option<&str>, key: Option<&str>) -> DeviceRef {
    DeviceRef {
        backend: backend.to_owned(),
        serial: serial.map(str::to_owned),
        key: key.map(str::to_owned),
    }
}

fn dial(stream: u32, port: Option<&str>, hz: f64) -> TunerDial {
    TunerDial {
        stream,
        port: port.map(str::to_owned),
        hz,
    }
}

#[test]
fn a_radio_is_named_by_whichever_identity_its_reference_carries() {
    assert_eq!(
        ref_label(&reference("rtlsdr", Some("00000001"), None)),
        "rtlsdr · 00000001"
    );
    assert_eq!(
        ref_label(&reference("virtual", None, Some("siggen"))),
        "virtual · siggen"
    );
    assert_eq!(
        ref_label(&reference("soapy", Some("123456"), Some("123456@DT"))),
        "soapy · 123456@DT"
    );
    assert_eq!(ref_label(&reference("hackrf", None, None)), "hackrf");
}

#[test]
fn lanes_are_drawn_only_when_every_stream_tunes_on_its_own() {
    let lanes = set(
        json!({ "capabilities": caps(json!({ "rx_streams": 5, "per_stream": { "tuning": true, "gain": true } })) }),
    );
    let shared = set(
        json!({ "capabilities": caps(json!({ "rx_streams": 4, "per_stream": { "gain": true } })) }),
    );
    assert!(lanes_merged(&lanes));
    assert!(!lanes_merged(&shared));
    assert!(!lanes_merged(&set(json!({}))));
}

#[test]
fn lane_controls_cover_only_what_a_lane_sets_on_its_own() {
    let gain = json!([{ "kind": "lna", "name": "LNA", "range": { "min": 0, "max": 40 } }]);
    let per_lane = json!({ "tuning": true, "gain": true });
    assert!(has_lane_controls(&caps(
        json!({ "per_stream": per_lane, "gains": gain })
    )));
    assert!(!has_lane_controls(&caps(json!({ "per_stream": per_lane }))));
    assert!(!has_lane_controls(&caps(json!({ "gains": gain }))));
    assert!(has_lane_controls(&caps(
        json!({ "per_stream": { "antenna": true }, "antennas": ["A", "B"] })
    )));
}

#[test]
fn the_bond_between_lanes_is_named_and_independent_lanes_have_none() {
    assert_eq!(bond_said(Coherence::TimeSync), Some("Shared clock"));
    assert_eq!(bond_said(Coherence::PhaseCoherent), Some("Phase coherent"));
    assert_eq!(bond_said(Coherence::None), None);
}

#[test]
fn a_single_stream_radio_and_a_shared_tuning_array_draw_one_unnamed_dial() {
    let one = set(json!({ "settings": { "center_hz": 100_000_000.0 } }));
    assert_eq!(tuner_dials(&one), vec![dial(0, None, 100e6)]);
    let array = set(json!({
        "capabilities": caps(json!({ "rx_streams": 4, "per_stream": { "gain": true } })),
        "settings": { "center_hz": 433_920_000.0 }
    }));
    assert_eq!(tuner_dials(&array), vec![dial(0, None, 433.92e6)]);
    let lone = set(json!({
        "capabilities": caps(json!({ "rx_streams": 1, "per_stream": { "tuning": true } })),
        "settings": { "center_hz": 100_000_000.0 }
    }));
    assert_eq!(tuner_dials(&lone), vec![dial(0, None, 100e6)]);
}

#[test]
fn a_radio_that_tunes_per_stream_draws_one_dial_per_port() {
    let two = set(json!({
        "capabilities": caps(json!({ "rx_streams": 2, "per_stream": { "tuning": true, "gain": true, "antenna": true } })),
        "settings": { "center_hz": 100_000_000.0, "streams": [{ "stream": 1, "center_hz": 433_920_000.0 }] }
    }));
    assert_eq!(
        tuner_dials(&two),
        vec![dial(0, Some("iq1"), 100e6), dial(1, Some("iq2"), 433.92e6)]
    );
}

#[test]
fn a_tune_moves_the_whole_radio_or_only_the_lane_touched() {
    let shared = caps(json!({ "rx_streams": 4 }));
    assert_eq!(
        tune_delta(&shared, 0, 145.5e6),
        DeviceSettings {
            center_hz: Some(145.5e6),
            tuning: Some(Tuning::Manual),
            ..DeviceSettings::default()
        }
    );
    let apart = caps(json!({ "rx_streams": 2, "per_stream": { "tuning": true } }));
    let delta = tune_delta(&apart, 1, 434e6);
    assert_eq!(
        delta.streams,
        vec![StreamSettings {
            stream: 1,
            center_hz: Some(434e6),
            tuning: Some(Tuning::Manual),
            ..StreamSettings::default()
        }]
    );
    let mut retuned = set(json!({
        "capabilities": apart,
        "settings": { "center_hz": 100e6, "streams": [{ "stream": 0, "center_hz": 101e6 }, { "stream": 1, "center_hz": 433.92e6 }] }
    }));
    retuned.settings.merge_from(&delta);
    assert_eq!(
        tuner_dials(&retuned),
        vec![dial(0, Some("iq1"), 101e6), dial(1, Some("iq2"), 434e6)]
    );
}

#[test]
fn a_fault_says_what_the_operator_can_do_about_it() {
    let unplugged = set(json!({ "status": "error", "fault": "unplugged", "error": "gone" }));
    assert_eq!(
        fault_said(&unplugged).as_deref(),
        Some(
            "Signal Generator is no longer attached. Plug it back in and it picks up where it left off."
        )
    );
    let busy = set(json!({ "status": "error", "fault": "in_use", "error": "busy" }));
    assert!(fault_said(&busy).is_some_and(|said| said.contains("open in another program")));
    let denied = set(json!({ "status": "error", "fault": "permissions", "error": "EACCES" }));
    assert!(fault_said(&denied).is_some_and(|said| said.contains("Check hardware")));
    assert_eq!(
        fault_said(&set(
            json!({ "status": "error", "fault": "other", "error": "boom" })
        )),
        None
    );
    assert_eq!(
        fault_said(&set(json!({ "status": "error", "error": "boom" }))),
        None
    );
}

#[test]
fn a_refusal_names_what_the_radio_would_not_take() {
    let refused = |names: Value| {
        set(json!({ "refused": { "settings": names, "error": "endpoint stalled" } }))
    };
    assert_eq!(
        refusal_said(&refused(json!(["frequency"]))).as_deref(),
        Some("Radio refused the new frequency")
    );
    assert_eq!(
        refusal_said(&refused(json!(["frequency", "gain", "AGC"]))).as_deref(),
        Some("Radio refused the new frequency, gain and AGC")
    );
    assert_eq!(
        refusal_said(&refused(json!([]))).as_deref(),
        Some("Radio refused the change")
    );
    assert_eq!(refusal_said(&set(json!({}))), None);
    let faulted = set(
        json!({ "status": "error", "error": "gone", "refused": { "settings": ["frequency"], "error": "x" } }),
    );
    assert_eq!(refusal_said(&faulted), None);
}

#[test]
fn a_radio_follows_the_decoders_until_the_operator_takes_the_wheel() {
    assert!(auto_tuning(&set(json!({})), 0));
    assert!(auto_tuning(
        &set(json!({ "settings": { "tuning": "auto" } })),
        0
    ));
    assert!(!auto_tuning(
        &set(json!({ "settings": { "tuning": "manual" } })),
        0
    ));
    let apart = set(json!({
        "capabilities": caps(json!({ "rx_streams": 2, "per_stream": { "tuning": true } })),
        "settings": { "streams": [{ "stream": 1, "tuning": "manual" }] }
    }));
    assert!(auto_tuning(&apart, 0));
    assert!(!auto_tuning(&apart, 1));
    let shared = set(json!({
        "capabilities": caps(json!({ "rx_streams": 2 })),
        "settings": { "streams": [{ "stream": 1, "tuning": "manual" }] }
    }));
    assert!(auto_tuning(&shared, 1));
}

#[test]
fn a_tuning_switch_covers_the_radio_or_only_the_stream_touched() {
    assert_eq!(
        tuning_delta(&caps(json!({ "rx_streams": 2 })), 1, Tuning::Manual),
        DeviceSettings {
            tuning: Some(Tuning::Manual),
            ..DeviceSettings::default()
        }
    );
    let apart = caps(json!({ "rx_streams": 2, "per_stream": { "tuning": true } }));
    assert_eq!(
        tuning_delta(&apart, 1, Tuning::Auto).streams,
        vec![StreamSettings {
            stream: 1,
            tuning: Some(Tuning::Auto),
            ..StreamSettings::default()
        }]
    );
}

#[test]
fn a_lock_holds_and_frees_one_stream_without_touching_the_others() {
    assert_eq!(lock_stream(&[], 1, true), vec![1]);
    assert_eq!(lock_stream(&[1], 0, true), vec![0, 1]);
    assert_eq!(lock_stream(&[0, 1], 0, false), vec![1]);
    assert_eq!(lock_stream(&[1], 1, true), vec![1]);
}

#[test]
fn hearing_is_green_yellow_or_red_by_how_many_decoders_sit_in_the_window() {
    assert_eq!(
        hearing(&set(json!({}))),
        Hearing {
            heard: 0,
            total: 0,
            tone: Tone::Ok
        }
    );
    assert_eq!(
        hearing(&set(
            json!({ "channels": channels(&[false, false, false]) })
        )),
        Hearing {
            heard: 3,
            total: 3,
            tone: Tone::Ok
        }
    );
    assert_eq!(
        hearing(&set(json!({ "channels": channels(&[false, true, true]) }))),
        Hearing {
            heard: 1,
            total: 3,
            tone: Tone::Warn
        }
    );
    assert_eq!(
        hearing(&set(json!({ "channels": channels(&[true, true]) }))),
        Hearing {
            heard: 0,
            total: 2,
            tone: Tone::Danger
        }
    );
    let manual =
        set(json!({ "settings": { "tuning": "manual" }, "channels": channels(&[false, true]) }));
    assert_eq!(
        hearing(&manual),
        Hearing {
            heard: 1,
            total: 2,
            tone: Tone::Warn
        }
    );
    let faulted = set(json!({ "status": "error", "channels": channels(&[false, false]) }));
    assert_eq!(
        hearing(&faulted),
        Hearing {
            heard: 2,
            total: 2,
            tone: Tone::Danger
        }
    );
}

#[test]
fn clipping_names_its_lanes_by_port_or_just_says_yes() {
    assert_eq!(clipping_said(&set(json!({}))), None);
    let bank = set(json!({ "capabilities": caps(json!({ "rx_streams": 5 })), "clipping": [0, 3] }));
    assert_eq!(clipping_said(&bank).as_deref(), Some("iq1, iq4"));
    assert_eq!(
        clipping_said(&set(json!({ "clipping": [0] }))).as_deref(),
        Some("yes")
    );
}

fn bank() -> Capabilities {
    caps(json!({
        "agc": { "kind": "switch" },
        "gains": [{ "name": "tuner", "kind": "tuner", "range": { "min": 0, "max": 49.6 } }],
        "rx_streams": 5,
        "per_stream": { "tuning": true, "gain": true, "agc": true }
    }))
}

#[test]
fn an_agc_switch_covers_one_lane_of_a_bank_and_the_whole_radio_otherwise() {
    assert_eq!(
        agc_delta(&bank(), 3, AgcSetting::switched(true)).streams,
        vec![StreamSettings {
            stream: 3,
            agc: Some(AgcSetting::switched(true)),
            ..StreamSettings::default()
        }]
    );
    assert_eq!(
        agc_delta(
            &caps(json!({ "agc": { "kind": "switch" } })),
            0,
            AgcSetting::switched(false)
        ),
        DeviceSettings {
            agc: Some(AgcSetting::switched(false)),
            ..DeviceSettings::default()
        }
    );
    let lanes = set(
        json!({ "capabilities": bank(), "settings": { "agc": { "on": false }, "streams": [{ "stream": 2, "agc": { "on": true } }] } }),
    );
    assert!(lane_agc(&lanes, 2).on);
    assert!(!lane_agc(&lanes, 1).on);
}

#[test]
fn the_settled_gain_shows_only_while_the_agc_runs() {
    let lanes = set(json!({
        "capabilities": bank(),
        "settings": { "streams": [{ "stream": 1, "agc": { "on": true } }] },
        "agc_gains": [{ "stream": 1, "value_db": 28.0 }, { "stream": 0, "value_db": 12.5 }]
    }));
    assert_eq!(agc_gain_db(&lanes, 1), Some(28.0));
    assert_eq!(agc_gain_db(&lanes, 0), None);
}

#[test]
fn the_agc_tip_reads_back_the_gain_and_advises_without_forcing() {
    let on = set(json!({
        "capabilities": caps(json!({
            "agc": { "kind": "switch" },
            "gains": [{ "name": "tuner", "kind": "tuner", "range": { "min": 0, "max": 49.6 } }]
        })),
        "settings": { "agc": { "on": true } },
        "agc_gains": [{ "stream": 0, "value_db": 28.0 }]
    }));
    assert_eq!(agc_tip(&on, 0, false), "AGC on at 28.0 dB");
    assert_eq!(
        agc_tip(&on, 0, true),
        "AGC on at 28.0 dB. Fixed gain keeps coherent lanes calibrated"
    );
    let off = set(json!({ "settings": { "agc": { "on": false } } }));
    assert_eq!(agc_tip(&off, 0, true), "AGC off, as coherent lanes want");
}

#[test]
fn coherent_lanes_are_those_a_coherent_node_or_an_array_uses() {
    let graph: PatchGraph = serde_json::from_value(json!({
        "nodes": [
            { "id": "kraken", "kind": "device", "data": {}, "position": { "x": 0, "y": 0 } },
            { "id": "bench", "kind": "array", "data": { "members": 1, "coherence": "time_sync", "shared_tuning": true }, "position": { "x": 0, "y": 0 } },
            { "id": "fm", "kind": "channel", "data": { "channel_type": "nfm" }, "position": { "x": 0, "y": 0 } }
        ],
        "edges": [
            { "from": { "node": "kraken", "port": "iq2" }, "to": { "node": "bench", "port": "iq" } },
            { "from": { "node": "kraken", "port": "iq4" }, "to": { "node": "bench", "port": "iq2" } },
            { "from": { "node": "kraken", "port": "iq" }, "to": { "node": "fm", "port": "iq" } }
        ]
    }))
    .expect("graph");
    assert_eq!(
        coherent_lanes(&graph, "kraken")
            .into_iter()
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
}
