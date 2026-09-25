use sdrmm_wire::{
    RecordingInfo,
    device::RECORDING_DRIVER_ID,
    patch::{NodeBody, PatchGraph},
};

use crate::ui::kit_sources::units;

pub const META_SUFFIX: &str = ".sigmf-meta";
pub const DATA_SUFFIX: &str = ".sigmf-data";

#[must_use]
pub fn recording_device_id(stem: &str) -> String {
    format!("{RECORDING_DRIVER_ID}:{stem}")
}

#[must_use]
pub fn find_recording<'a>(
    library: &'a [RecordingInfo],
    stem: Option<&str>,
) -> Option<&'a RecordingInfo> {
    let stem = stem.filter(|stem| !stem.is_empty())?;
    library.iter().find(|recording| recording.file == stem)
}

#[must_use]
pub fn claimed_recordings(graph: &PatchGraph, except: &str) -> Vec<String> {
    graph
        .nodes
        .iter()
        .filter(|node| node.id != except)
        .filter_map(|node| match &node.body {
            NodeBody::Recording(recording) => {
                recording.recording.clone().filter(|stem| !stem.is_empty())
            }
            _ => None,
        })
        .collect()
}

#[must_use]
pub fn recording_title(recording: &RecordingInfo) -> String {
    match recording.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => recording.file.clone(),
    }
}

#[must_use]
pub fn matches_search(recording: &RecordingInfo, search: &str) -> bool {
    let needle = search.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    [
        Some(recording.file.as_str()),
        recording.name.as_deref(),
        recording.note.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(recording.tags.iter().map(String::as_str))
    .any(|field| field.to_lowercase().contains(&needle))
}

#[must_use]
pub fn recording_choices(
    library: &[RecordingInfo],
    claimed: &[String],
    search: &str,
) -> Vec<RecordingInfo> {
    let mut choices: Vec<RecordingInfo> = library
        .iter()
        .filter(|recording| !claimed.contains(&recording.file))
        .filter(|recording| matches_search(recording, search))
        .cloned()
        .collect();
    choices.sort_by_key(|recording| recording_title(recording).to_lowercase());
    choices
}

#[must_use]
pub fn describe_recording(recording: &RecordingInfo) -> String {
    [
        units::mhz(recording.center_hz),
        units::sample_rate(recording.sample_rate),
        units::duration(recording.duration_s),
        units::bytes(recording.bytes as f64),
    ]
    .join(" · ")
}

#[must_use]
pub fn recording_provenance(recording: &RecordingInfo) -> String {
    let mut parts = vec![recording.created_at.clone(), recording.device_label.clone()];
    if recording_title(recording) != recording.file {
        parts.push(recording.file.clone());
    }
    parts.extend(recording.tags.iter().map(|tag| format!("#{tag}")));
    parts.join(" · ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadProblem {
    Empty,
    LoneMeta,
    LoneData,
    Mixed,
}

impl UploadProblem {
    #[must_use]
    pub const fn said(self) -> &'static str {
        match self {
            Self::Empty => "Pick a .sigmf archive, or a .sigmf-meta and .sigmf-data pair.",
            Self::LoneMeta => "That is the metadata on its own: add the matching .sigmf-data.",
            Self::LoneData => "That is the samples on their own: add the matching .sigmf-meta.",
            Self::Mixed => "Send one .sigmf archive, or one .sigmf-meta with one .sigmf-data.",
        }
    }
}

#[must_use]
pub fn check_upload(names: &[String]) -> Option<UploadProblem> {
    if names.is_empty() {
        return Some(UploadProblem::Empty);
    }
    let meta = names
        .iter()
        .filter(|name| name.ends_with(META_SUFFIX))
        .count();
    let data = names
        .iter()
        .filter(|name| name.ends_with(DATA_SUFFIX))
        .count();
    let archives = names.len() - meta - data;
    if archives > 0 {
        return (archives != names.len() || archives != 1).then_some(UploadProblem::Mixed);
    }
    if meta == 1 && data == 1 {
        return None;
    }
    Some(if data == 0 {
        UploadProblem::LoneMeta
    } else {
        UploadProblem::LoneData
    })
}

#[must_use]
pub fn upload_field(name: &str) -> &'static str {
    if name.ends_with(META_SUFFIX) {
        "meta"
    } else if name.ends_with(DATA_SUFFIX) {
        "data"
    } else {
        "archive"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn recording(file: &str, extra: serde_json::Value) -> RecordingInfo {
        let mut base = json!({
            "id": file.len(), "file": file, "device_id": recording_device_id(file),
            "device_label": "RTL-SDR 0", "center_hz": 100e6, "sample_rate": 2.048e6,
            "samples": 2_048_000, "bytes": 16_384_000, "duration_s": 1.0,
            "created_at": "2026-09-16T12:00:00Z"
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::from_value(base).expect("recording")
    }

    fn files(recordings: &[RecordingInfo]) -> Vec<&str> {
        recordings
            .iter()
            .map(|recording| recording.file.as_str())
            .collect()
    }

    #[test]
    fn a_recording_is_named_by_its_stem_alone() {
        assert_eq!(
            recording_device_id("2026-09-16T10-00-00_rtlsdr"),
            "recording:2026-09-16T10-00-00_rtlsdr"
        );
    }

    #[test]
    fn other_recording_nodes_claim_what_they_play() {
        let graph: PatchGraph = serde_json::from_value(json!({
            "nodes": [
                { "id": "a", "kind": "recording", "data": { "recording": "airband" }, "position": { "x": 0, "y": 0 } },
                { "id": "b", "kind": "recording", "data": { "recording": "weather" }, "position": { "x": 0, "y": 0 } },
                { "id": "c", "kind": "recording", "data": {}, "position": { "x": 0, "y": 0 } },
                { "id": "d", "kind": "device", "data": {}, "position": { "x": 0, "y": 0 } }
            ],
            "edges": []
        }))
        .expect("graph");
        assert_eq!(claimed_recordings(&graph, "a"), vec!["weather"]);
        assert_eq!(claimed_recordings(&graph, "z"), vec!["airband", "weather"]);
    }

    #[test]
    fn choices_are_free_recordings_sorted_by_name_and_searchable() {
        let library = [
            recording(
                "airband",
                json!({ "name": "Tower watch", "tags": ["airband"] }),
            ),
            recording("weather", json!({ "note": "80 m net" })),
            recording("zulu", json!({})),
        ];
        assert_eq!(
            files(&recording_choices(&library, &["weather".to_owned()], "")),
            vec!["airband", "zulu"]
        );
        assert_eq!(
            files(&recording_choices(&library, &[], "tower")),
            vec!["airband"]
        );
        assert_eq!(
            files(&recording_choices(&library, &[], "80 m")),
            vec!["weather"]
        );
        assert!(recording_choices(&library, &[], "marine").is_empty());
    }

    #[test]
    fn a_recording_is_found_by_stem_and_a_gone_one_is_not() {
        let library = [recording("airband", json!({}))];
        assert_eq!(
            find_recording(&library, Some("airband")).map(|r| r.file.as_str()),
            Some("airband")
        );
        assert!(find_recording(&library, Some("gone")).is_none());
        assert!(find_recording(&library, None).is_none());
        assert!(find_recording(&library, Some("")).is_none());
    }

    #[test]
    fn an_upload_is_one_archive_or_one_meta_with_one_data() {
        let names = |list: &[&str]| {
            list.iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(check_upload(&names(&["take.sigmf"])), None);
        assert_eq!(
            check_upload(&names(&["take.sigmf-meta", "take.sigmf-data"])),
            None
        );
        assert_eq!(
            check_upload(&names(&["take.sigmf-data", "take.sigmf-meta"])),
            None
        );
        assert_eq!(check_upload(&names(&[])), Some(UploadProblem::Empty));
        assert_eq!(
            check_upload(&names(&["take.sigmf-meta"])),
            Some(UploadProblem::LoneMeta)
        );
        assert_eq!(
            check_upload(&names(&["take.sigmf-data"])),
            Some(UploadProblem::LoneData)
        );
        assert_eq!(
            check_upload(&names(&["a.sigmf-meta", "b.sigmf-meta"])),
            Some(UploadProblem::LoneMeta)
        );
        assert_eq!(
            check_upload(&names(&["a.sigmf", "b.sigmf"])),
            Some(UploadProblem::Mixed)
        );
        assert_eq!(
            check_upload(&names(&["a.sigmf", "a.sigmf-meta"])),
            Some(UploadProblem::Mixed)
        );
    }

    #[test]
    fn each_file_goes_up_under_the_field_its_suffix_names() {
        assert_eq!(upload_field("take.sigmf-meta"), "meta");
        assert_eq!(upload_field("take.sigmf-data"), "data");
        assert_eq!(upload_field("take.sigmf"), "archive");
    }

    #[test]
    fn a_recording_reads_as_centre_rate_length_and_size() {
        assert_eq!(
            describe_recording(&recording("airband", json!({}))),
            "100.0000 MHz · 2.048 MS/s · 1.0 s · 16.384 MB"
        );
        assert_eq!(
            recording_title(&recording("airband", json!({ "name": "  " }))),
            "airband"
        );
        assert_eq!(
            recording_title(&recording("airband", json!({ "name": "Tower" }))),
            "Tower"
        );
    }
}
