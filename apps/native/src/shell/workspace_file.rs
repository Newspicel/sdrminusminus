use sdrmm_wire::{
    workspace::{MAX_NAME_LEN, WorkspaceExport},
    workspace_state::WorkspaceState,
};
use serde_json::Value;

pub const DEFAULT_NAME: &str = "Workspace";

pub const NOT_JSON: &str = "That file is not a workspace: it is not JSON.";
pub const NOT_WORKSPACE: &str = "That file is not a workspace.";

#[must_use]
pub fn workspace_name(typed: &str) -> String {
    let cut: String = typed.trim().chars().take(MAX_NAME_LEN).collect();
    let trimmed = cut.trim_end();
    if trimmed.is_empty() {
        DEFAULT_NAME.to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[must_use]
pub fn export_file_name(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        String::from("workspace.json")
    } else {
        format!("workspace-{slug}.json")
    }
}

pub fn parse_export(text: &str) -> Result<WorkspaceExport, &'static str> {
    let document: Value = serde_json::from_str(text).map_err(|_| NOT_JSON)?;
    let version = document
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or(NOT_WORKSPACE)?;
    let name = document
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or(NOT_WORKSPACE)?;
    let snapshot = document
        .get("snapshot")
        .filter(|snapshot| {
            snapshot
                .get("graph")
                .and_then(|graph| graph.get("nodes"))
                .is_some_and(Value::is_array)
        })
        .ok_or(NOT_WORKSPACE)?;
    let snapshot =
        sdrmm_server::parse_workspace_snapshot(&snapshot.to_string()).map_err(|_| NOT_WORKSPACE)?;
    let state = match document.get("state") {
        Some(state) => {
            serde_json::from_value::<WorkspaceState>(state.clone()).map_err(|_| NOT_WORKSPACE)?
        }
        None => WorkspaceState::new(),
    };
    Ok(WorkspaceExport {
        version,
        name: name.to_owned(),
        snapshot,
        state,
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::PortRef;
    use serde_json::json;

    use super::*;

    fn document() -> Value {
        json!({
            "version": 1,
            "name": "Airband Watch",
            "snapshot": {
                "version": 3,
                "graph": {
                    "nodes": [
                        { "id": "dev", "kind": "device", "data": {}, "position": { "x": 0, "y": 0 } },
                        { "id": "scope", "kind": "scope", "position": { "x": 400, "y": 0 } }
                    ],
                    "edges": [
                        { "from": { "node": "dev", "port": "iq" }, "to": { "node": "scope", "port": "iq" } }
                    ]
                }
            },
            "state": { "version": 2, "devices": [{ "node": "dev", "settings": { "center_hz": 145_500_000.0 } }] }
        })
    }

    fn port(node: &str, port: &str) -> PortRef {
        PortRef {
            node: node.to_owned(),
            port: port.to_owned(),
        }
    }

    #[test]
    fn keeps_what_was_typed() {
        assert_eq!(workspace_name("Bench"), "Bench");
    }

    #[test]
    fn trims_the_edges_the_server_would_reject() {
        assert_eq!(workspace_name("  Bench  "), "Bench");
    }

    #[test]
    fn falls_back_to_the_default_when_nothing_was_typed() {
        assert_eq!(workspace_name(""), DEFAULT_NAME);
        assert_eq!(workspace_name("   "), DEFAULT_NAME);
    }

    #[test]
    fn cuts_a_name_the_server_would_refuse_for_its_length() {
        assert_eq!(
            workspace_name(&"x".repeat(MAX_NAME_LEN + 10)),
            "x".repeat(MAX_NAME_LEN)
        );
    }

    #[test]
    fn names_the_export_file_after_the_workspace() {
        assert_eq!(
            export_file_name("Airband Watch"),
            "workspace-airband-watch.json"
        );
        assert_eq!(export_file_name("!!"), "workspace.json");
    }

    #[test]
    fn keeps_the_name_the_layout_and_the_tuning_the_file_was_written_with() {
        let read = parse_export(&document().to_string()).expect("a workspace");
        assert_eq!(read.name, "Airband Watch");
        assert_eq!(read.version, 1);
        assert_eq!(read.snapshot.graph.nodes.len(), 2);
        assert_eq!(read.snapshot.graph.edges[0].to, port("scope", "iq"));
        assert_eq!(
            read.state.devices[0].settings.center_hz,
            Some(145_500_000.0)
        );
    }

    #[test]
    fn lifts_a_control_wire_into_a_radio_onto_the_decoder_it_feeds() {
        let mut older = document();
        older["snapshot"]["graph"]["nodes"]
            .as_array_mut()
            .expect("nodes")
            .extend([
                json!({ "id": "nfm", "kind": "channel", "data": { "channel_type": "nfm" }, "position": { "x": 0, "y": 200 } }),
                json!({ "id": "scan", "kind": "scanner", "position": { "x": 0, "y": 400 } }),
            ]);
        older["snapshot"]["graph"]["edges"]
            .as_array_mut()
            .expect("edges")
            .extend([
                json!({ "from": { "node": "dev", "port": "iq" }, "to": { "node": "nfm", "port": "iq" } }),
                json!({ "from": { "node": "scan", "port": "control" }, "to": { "node": "dev", "port": "control" } }),
            ]);

        let read = parse_export(&older.to_string()).expect("a workspace");
        let wire = read
            .snapshot
            .graph
            .edges
            .iter()
            .find(|edge| edge.from.node == "scan")
            .expect("the scanner stays wired");
        assert_eq!(wire.to, port("nfm", "control"));
    }

    #[test]
    fn refuses_a_file_that_is_not_a_workspace() {
        assert_eq!(parse_export("not json").err(), Some(NOT_JSON));
        assert_eq!(parse_export("[]").err(), Some(NOT_WORKSPACE));
        assert_eq!(
            parse_export(&json!({ "version": 1, "name": "No layout" }).to_string()).err(),
            Some(NOT_WORKSPACE)
        );
        let mut unnamed = document();
        unnamed["name"] = json!("  ");
        assert_eq!(
            parse_export(&unnamed.to_string()).err(),
            Some(NOT_WORKSPACE)
        );
    }
}
