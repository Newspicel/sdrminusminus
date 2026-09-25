use sdrmm_wire::{
    patch::{NodeBody, PatchGraph, PatchNode, Position, RecordingNode},
    rest::{
        AudioRecordingInfo, MAX_RECORDING_NAME_LEN, MAX_RECORDING_TAG_LEN, MAX_RECORDING_TAGS,
        RecordingAnnotation, RecordingInfo,
    },
    units,
    workspace::MAX_NAME_LEN,
};

pub const META_SUFFIX: &str = ".sigmf-meta";
pub const DATA_SUFFIX: &str = ".sigmf-data";

pub const DOWNLOAD_FORMATS: [(&str, &str); 2] = [(".sigmf", "sigmf"), (".wav", "wav")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadProblem {
    Empty,
    LoneMeta,
    LoneData,
    Mixed,
}

impl UploadProblem {
    #[must_use]
    pub fn said(self) -> &'static str {
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
        return (archives != 1 || names.len() != 1).then_some(UploadProblem::Mixed);
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

#[must_use]
pub fn format_mhz(hz: f64) -> String {
    format!("{:.4} MHz", hz / 1e6)
}

#[must_use]
pub fn parse_tags(input: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for raw in input.split(',') {
        let tag: String = raw.trim().chars().take(MAX_RECORDING_TAG_LEN).collect();
        if !tag.is_empty()
            && !tags
                .iter()
                .any(|kept| kept.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag);
        }
    }
    tags.truncate(MAX_RECORDING_TAGS);
    tags
}

#[must_use]
pub fn format_tags(tags: &[String]) -> String {
    tags.join(", ")
}

#[must_use]
pub fn annotation(name: &str, tags: &str, note: &str) -> RecordingAnnotation {
    let name = name.trim();
    let note = note.trim();
    RecordingAnnotation {
        name: (!name.is_empty()).then(|| name.chars().take(MAX_RECORDING_NAME_LEN).collect()),
        tags: parse_tags(tags),
        note: (!note.is_empty()).then(|| note.to_owned()),
    }
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
pub fn format_duration(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() / 10.0;
    if tenths < 60.0 {
        return format!("{tenths:.1} s");
    }
    let whole = tenths.round() as u64;
    let (h, m, s) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[must_use]
pub fn format_recorded_at(created_at: &str) -> Option<String> {
    let at: jiff::Timestamp = created_at.parse().ok()?;
    let local = at.to_zoned(jiff::tz::TimeZone::system());
    Some(local.strftime("%b %-d, %Y, %H:%M").to_string())
}

#[must_use]
pub fn describe_recording(recording: &RecordingInfo) -> String {
    [
        format_mhz(recording.center_hz),
        units::sample_rate(recording.sample_rate),
        format_duration(recording.duration_s),
        units::bytes(recording.bytes as f64),
    ]
    .join(" \u{b7} ")
}

#[must_use]
pub fn describe_audio(recording: &AudioRecordingInfo) -> String {
    [
        String::from(if recording.channels == 2 {
            "stereo"
        } else {
            "mono"
        }),
        units::sample_rate(f64::from(recording.sample_rate)),
        format_duration(recording.duration_s),
        units::bytes(recording.bytes as f64),
    ]
    .join(" \u{b7} ")
}

#[must_use]
pub fn recording_provenance(recording: &RecordingInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(format_recorded_at(&recording.created_at));
    parts.push(recording.device_label.clone());
    if recording_title(recording) != recording.file {
        parts.push(recording.file.clone());
    }
    parts.extend(recording.tags.iter().map(|tag| format!("#{tag}")));
    parts.join(" \u{b7} ")
}

#[must_use]
pub fn open_position(graph: &PatchGraph) -> Position {
    let right = graph
        .nodes
        .iter()
        .map(|node| node.position.x + node.size.map_or(320.0, |size| size.w))
        .fold(f32::MIN, f32::max);
    let top = graph
        .nodes
        .iter()
        .map(|node| node.position.y)
        .fold(f32::MAX, f32::min);
    if graph.nodes.is_empty() {
        Position { x: 40.0, y: 40.0 }
    } else {
        Position {
            x: right + 60.0,
            y: top,
        }
    }
}

#[must_use]
pub fn recording_node_for(recording: &RecordingInfo, id: String, position: Position) -> PatchNode {
    PatchNode {
        id,
        body: NodeBody::Recording(RecordingNode {
            recording: Some(recording.file.clone()),
        }),
        position,
        size: None,
        label: Some(
            recording_title(recording)
                .chars()
                .take(MAX_NAME_LEN)
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> RecordingInfo {
        RecordingInfo {
            id: 1,
            file: "siggen-20260809-120000".to_owned(),
            name: None,
            device_id: "virtual:file:/recordings/siggen".to_owned(),
            device_label: "Signal Generator".to_owned(),
            center_hz: 100e6,
            sample_rate: 2.048e6,
            samples: 4_096_000,
            bytes: 32_768_000,
            duration_s: 2.0,
            created_at: "2026-08-09T12:00:00Z".to_owned(),
            tags: vec!["airband".to_owned(), "tower".to_owned()],
            note: Some("EDDF ground".to_owned()),
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn shows_tenths_below_a_minute_then_clock_time() {
        assert_eq!(format_duration(0.0), "0.0 s");
        assert_eq!(format_duration(3.24), "3.2 s");
        assert_eq!(format_duration(59.99), "1:00");
        assert_eq!(format_duration(60.0), "1:00");
        assert_eq!(format_duration(3_599.0), "59:59");
        assert_eq!(format_duration(3_600.0), "1:00:00");
        assert_eq!(format_duration(7_325.0), "2:02:05");
    }

    #[test]
    fn scales_bytes_through_the_decimal_prefixes() {
        assert_eq!(units::bytes(0.0), "0 B");
        assert_eq!(units::bytes(999.0), "999 B");
        assert_eq!(units::bytes(1_000.0), "1 kB");
        assert_eq!(units::bytes(19_200_000.0), "19.2 MB");
        assert_eq!(units::bytes(2_500_000_000.0), "2.5 GB");
    }

    #[test]
    fn trims_tags_drops_blanks_and_keeps_the_first_spelling() {
        assert_eq!(
            parse_tags(" airband , AIRBAND ,, tower"),
            ["airband", "tower"]
        );
        assert!(parse_tags("   ").is_empty());
        assert_eq!(format_tags(&parse_tags("airband,tower")), "airband, tower");
        assert_eq!(
            parse_tags(&"t".repeat(MAX_RECORDING_TAG_LEN + 5)),
            ["t".repeat(MAX_RECORDING_TAG_LEN)]
        );
        let many = (0..MAX_RECORDING_TAGS + 4)
            .map(|at| format!("t{at}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(parse_tags(&many).len(), MAX_RECORDING_TAGS);
    }

    #[test]
    fn an_annotation_drops_blank_fields() {
        let blank = annotation("  ", "", "  ");
        assert_eq!(blank, RecordingAnnotation::default());
        let filled = annotation(" Tower ", "a, b", " note ");
        assert_eq!(filled.name.as_deref(), Some("Tower"));
        assert_eq!(filled.tags, ["a", "b"]);
        assert_eq!(filled.note.as_deref(), Some("note"));
    }

    #[test]
    fn searches_the_name_file_tags_and_note() {
        let named = RecordingInfo {
            name: Some("Tower watch".to_owned()),
            ..recording()
        };
        assert!(matches_search(&named, "tower wat"));
        assert!(!matches_search(&recording(), "tower watch"));
        assert!(matches_search(&recording(), ""));
        assert!(matches_search(&recording(), "  "));
        assert!(matches_search(&recording(), "SIGGEN"));
        assert!(matches_search(&recording(), "tow"));
        assert!(matches_search(&recording(), "eddf"));
        assert!(!matches_search(&recording(), "meteor"));
        let bare = RecordingInfo {
            tags: Vec::new(),
            note: None,
            ..recording()
        };
        assert!(!matches_search(&bare, "airband"));
        assert!(matches_search(&bare, "siggen"));
    }

    #[test]
    fn reads_out_what_the_capture_holds_and_where_it_came_from() {
        assert_eq!(
            describe_recording(&recording()),
            "100.0000 MHz \u{b7} 2.048 MS/s \u{b7} 2.0 s \u{b7} 32.768 MB"
        );
        let provenance = recording_provenance(&recording());
        assert!(provenance.contains("Signal Generator"));
        assert!(provenance.contains("#airband"));
        assert!(!provenance.starts_with("Signal Generator"));
        let undated = RecordingInfo {
            created_at: "whenever".to_owned(),
            tags: Vec::new(),
            ..recording()
        };
        assert_eq!(recording_provenance(&undated), "Signal Generator");
    }

    #[test]
    fn prefers_the_name_and_keeps_the_file_in_the_provenance() {
        assert_eq!(recording_title(&recording()), "siggen-20260809-120000");
        let named = RecordingInfo {
            name: Some("Tower watch".to_owned()),
            ..recording()
        };
        assert_eq!(recording_title(&named), "Tower watch");
        let blank = RecordingInfo {
            name: Some("  ".to_owned()),
            ..recording()
        };
        assert_eq!(recording_title(&blank), "siggen-20260809-120000");
        assert!(recording_provenance(&named).contains("siggen-20260809-120000"));
        assert!(!recording_provenance(&recording()).contains("siggen-20260809-120000"));
    }

    #[test]
    fn sends_each_file_in_the_slot_its_suffix_names() {
        assert_eq!(upload_field("a.sigmf-meta"), "meta");
        assert_eq!(upload_field("a.sigmf-data"), "data");
        assert_eq!(upload_field("a.sigmf"), "archive");
    }

    #[test]
    fn checks_an_upload_is_one_archive_or_one_pair() {
        assert_eq!(check_upload(&[]), Some(UploadProblem::Empty));
        assert_eq!(check_upload(&names(&["a.sigmf"])), None);
        assert_eq!(
            check_upload(&names(&["a.sigmf-meta", "a.sigmf-data"])),
            None
        );
        assert_eq!(
            check_upload(&names(&["a.sigmf-meta"])),
            Some(UploadProblem::LoneMeta)
        );
        assert_eq!(
            check_upload(&names(&["a.sigmf-data"])),
            Some(UploadProblem::LoneData)
        );
        assert_eq!(
            check_upload(&names(&["a.sigmf", "b.sigmf-meta"])),
            Some(UploadProblem::Mixed)
        );
    }

    #[test]
    fn opens_a_recording_as_a_labelled_source_beside_the_patch() {
        let node = recording_node_for(
            &recording(),
            "recording".to_owned(),
            open_position(&PatchGraph::default()),
        );
        assert_eq!(node.label.as_deref(), Some("siggen-20260809-120000"));
        assert!(matches!(
            node.body,
            NodeBody::Recording(RecordingNode { recording: Some(ref stem) }) if stem == "siggen-20260809-120000"
        ));
        let graph = PatchGraph {
            nodes: vec![node],
            edges: Vec::new(),
        };
        assert_eq!(open_position(&graph).x, 420.0);
    }
}
