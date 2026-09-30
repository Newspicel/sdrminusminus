use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    array::{ARRAY_FAILURE_LABELS, CalPhase, ProcessorGate, SyncState},
    patch::REFUSALS,
    phone::PHONE_TEXTS,
    radar::RadarProblem,
    ws::SurfaceRefusal,
};

pub const RADAR_REFUSED_TEMPLATE: &str = "{text}";

pub fn generated() -> Result<Value, serde_json::Error> {
    let mut labels = Map::new();
    labels.insert(
        "sync".to_owned(),
        keyed(SyncState::ALL.iter().map(|state| (state, state.label())))?,
    );
    labels.insert(
        "cal".to_owned(),
        keyed(CalPhase::ALL.iter().map(|phase| (phase, phase.label())))?,
    );
    labels.insert(
        "gate".to_owned(),
        keyed(ProcessorGate::ALL.iter().map(|gate| (gate, gate.label())))?,
    );
    labels.insert("failure".to_owned(), named(ARRAY_FAILURE_LABELS));
    labels.insert("radar_problem".to_owned(), named(radar_problems()));
    labels.insert("refusal".to_owned(), named(REFUSALS));
    labels.insert("phone".to_owned(), named(PHONE_TEXTS));
    labels.insert(
        "surface_refusal".to_owned(),
        keyed(
            SurfaceRefusal::ALL
                .iter()
                .map(|reason| (reason, reason.label())),
        )?,
    );
    Ok(Value::Object(labels))
}

fn radar_problems() -> Vec<(&'static str, String)> {
    RadarProblem::ALL
        .iter()
        .map(|problem| {
            let label = match problem {
                RadarProblem::Refused(_) => RADAR_REFUSED_TEMPLATE,
                other => other.label(),
            };
            (problem.kind(), label.to_owned())
        })
        .collect()
}

fn keyed<'a, T: Serialize + 'a>(
    rows: impl Iterator<Item = (&'a T, &'static str)>,
) -> Result<Value, serde_json::Error> {
    let mut table = Map::new();
    for (value, label) in rows {
        let key = match serde_json::to_value(value)? {
            Value::String(key) => key,
            other => other.to_string(),
        };
        table.insert(key, Value::String(label.to_owned()));
    }
    Ok(Value::Object(table))
}

fn named<L: Into<String>>(rows: impl IntoIterator<Item = (&'static str, L)>) -> Value {
    Value::Object(
        rows.into_iter()
            .map(|(key, label)| (key.to_owned(), Value::String(label.into())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const WRITTEN: &str = include_str!("../../../web/src/generated/labels.json");

    #[test]
    fn labels_json_matches_the_rust_labels() {
        let written: Value = serde_json::from_str(WRITTEN).expect("labels.json parses");
        assert_eq!(
            written,
            generated().expect("labels"),
            "run cargo xtask codegen"
        );
    }

    #[test]
    fn every_status_label_is_keyed_by_its_wire_name() {
        let labels = generated().expect("labels");
        assert_eq!(labels["sync"]["searching"], "Syncing");
        assert_eq!(labels["cal"]["failed"], "Cal failed");
        assert_eq!(labels["gate"]["tuning_mode"], "Wrong tuning");
        assert_eq!(labels["failure"]["clock_drift"], "Clocks drift {ppm} ppm");
        assert_eq!(labels["refusal"]["lane_taken"], "that lane is in {label}");
        assert_eq!(labels["radar_problem"]["refused"], "{text}");
        assert_eq!(labels["phone"]["offline"], "phone offline");
        assert_eq!(labels["phone"]["not_paired"], "phone not paired");
        assert_eq!(labels["surface_refusal"]["no_surface"], "No surface");
        for (section, count) in [
            ("sync", SyncState::ALL.len()),
            ("cal", CalPhase::ALL.len()),
            ("gate", ProcessorGate::ALL.len()),
            ("failure", ARRAY_FAILURE_LABELS.len()),
            ("radar_problem", RadarProblem::ALL.len()),
            ("refusal", REFUSALS.len()),
            ("phone", PHONE_TEXTS.len()),
            ("surface_refusal", SurfaceRefusal::ALL.len()),
        ] {
            assert_eq!(
                labels[section].as_object().map(Map::len),
                Some(count),
                "{section}"
            );
        }
    }

    #[test]
    fn surface_and_phone_labels_follow_the_wire() {
        let labels = generated().expect("labels");
        for reason in SurfaceRefusal::ALL {
            let key = serde_json::to_value(reason).expect("reason");
            let key = key.as_str().expect("a snake case name");
            assert_eq!(labels["surface_refusal"][key], reason.label());
        }
        for (key, text) in PHONE_TEXTS {
            assert_eq!(labels["phone"][key], text);
        }
    }

    #[test]
    fn radar_labels_follow_the_problem_labels() {
        let labels = generated().expect("labels");
        for problem in RadarProblem::ALL {
            if matches!(problem, RadarProblem::Refused(_)) {
                continue;
            }
            assert_eq!(labels["radar_problem"][problem.kind()], problem.label());
        }
        let refused = RadarProblem::Refused("Reference too weak".to_owned());
        assert_eq!(
            RADAR_REFUSED_TEMPLATE.replace("{text}", refused.label()),
            "Reference too weak"
        );
    }
}
