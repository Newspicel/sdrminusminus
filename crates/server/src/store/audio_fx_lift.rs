use rusqlite::{Connection, params};
use sdrmm_wire::{AudioAgcMode, AudioProcessing, MAX_EDGES, MAX_NODES};
use serde_json::{Value, json};

use super::{StoreError, edge_end};

const FX_OFFSET_X: f64 = 420.0;

struct Lifted {
    node: String,
    chain: AudioProcessing,
}

pub(super) fn lift_audio_chains(conn: &Connection) -> Result<(), StoreError> {
    let rows: Vec<(i64, String)> = conn
        .prepare("SELECT workspace_id, state FROM workspace_state WHERE state LIKE '%\"audio\"%'")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (id, state_json) in rows {
        let mut state: Value = serde_json::from_str(&state_json)?;
        let lifted = take_chains(&mut state);
        if lifted.is_none() {
            continue;
        }
        let snapshot_json: Option<String> = conn
            .query_row(
                "SELECT snapshot FROM workspaces WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        let tx = conn.unchecked_transaction()?;
        if let Some(snapshot_json) = snapshot_json {
            let mut snapshot: Value = serde_json::from_str(&snapshot_json)?;
            for chain in lifted.into_iter().flatten() {
                insert_fx(&mut snapshot, &chain);
            }
            let nodes = snapshot["graph"]["nodes"].as_array().map_or(0, Vec::len);
            tx.execute(
                "UPDATE workspaces SET snapshot = ?2, nodes = ?3, revision = revision + 1 \
                 WHERE id = ?1",
                params![id, serde_json::to_string(&snapshot)?, nodes as i64],
            )?;
        }
        tx.execute(
            "UPDATE workspace_state SET state = ?2 WHERE workspace_id = ?1",
            params![id, serde_json::to_string(&state)?],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn take_chains(state: &mut Value) -> Option<Vec<Lifted>> {
    let channels = state.get_mut("channels")?.as_array_mut()?;
    let mut touched = false;
    let mut lifted = Vec::new();
    for channel in channels {
        let Some(node) = channel
            .get("node")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(settings) = channel.get_mut("settings").and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(audio) = settings.remove("audio") else {
            continue;
        };
        touched = true;
        if let Some(blanker) = audio.get("blanker") {
            settings.entry("blanker").or_insert_with(|| blanker.clone());
        }
        let type_id = settings
            .get("params")
            .and_then(|params| params.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Ok(chain) = serde_json::from_value::<AudioProcessing>(audio) else {
            continue;
        };
        if worth_a_node(&chain, type_id) {
            lifted.push(Lifted { node, chain });
        }
    }
    touched.then_some(lifted)
}

fn worth_a_node(chain: &AudioProcessing, type_id: &str) -> bool {
    let levelled_by_its_demod =
        matches!(type_id, "am" | "ssb") && chain.agc == AudioAgcMode::Medium;
    let rest = AudioProcessing {
        agc: if levelled_by_its_demod {
            AudioAgcMode::Off
        } else {
            chain.agc
        },
        ..chain.clone()
    };
    rest.is_active() && rest.validate().is_ok()
}

fn insert_fx(snapshot: &mut Value, lifted: &Lifted) {
    let Some(graph) = snapshot.get_mut("graph") else {
        return;
    };
    let node_count = graph["nodes"].as_array().map_or(0, Vec::len);
    let edge_count = graph["edges"].as_array().map_or(0, Vec::len);
    if node_count >= MAX_NODES || edge_count >= MAX_EDGES {
        tracing::warn!(node = %lifted.node, "no room to lift a channel's audio chain into an audio FX node");
        return;
    }
    let Some(channel) = graph["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node.get("id").and_then(Value::as_str) == Some(&lifted.node))
        })
        .cloned()
    else {
        return;
    };
    let id = free_id(graph, &format!("audio_fx:{}", lifted.node));
    let x = channel["position"]["x"].as_f64().unwrap_or_default() + FX_OFFSET_X;
    let y = channel["position"]["y"].as_f64().unwrap_or_default();
    let Some(nodes) = graph["nodes"].as_array_mut() else {
        return;
    };
    nodes.push(json!({
        "id": id,
        "kind": "audio_fx",
        "data": { "settings": lifted.chain },
        "position": { "x": x, "y": y },
    }));
    let Some(edges) = graph["edges"].as_array_mut() else {
        return;
    };
    for edge in edges.iter_mut() {
        if edge_end(edge, "from") == Some((lifted.node.as_str(), "audio")) {
            edge["from"]["node"] = json!(id);
        }
    }
    edges.push(json!({
        "from": { "node": lifted.node, "port": "audio" },
        "to": { "node": id, "port": "audio" },
    }));
}

fn free_id(graph: &Value, wanted: &str) -> String {
    let taken = |id: &str| {
        graph["nodes"].as_array().is_some_and(|nodes| {
            nodes
                .iter()
                .any(|node| node.get("id").and_then(Value::as_str) == Some(id))
        })
    };
    let mut id = wanted.to_owned();
    let mut n = 2;
    while taken(&id) {
        id = format!("{wanted}:{n}");
        n += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{NodeBody, WorkspaceSnapshot, WorkspaceState};

    use super::*;

    fn store_with(snapshot: &Value, state: &Value) -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        super::super::migrate(&conn).expect("schema");
        conn.execute(
            "INSERT INTO workspaces (id, name, created_at, updated_at, revision, nodes, snapshot) \
             VALUES (1, 'w', 't', 't', 1, 3, ?1)",
            params![snapshot.to_string()],
        )
        .expect("workspace row");
        conn.execute(
            "INSERT INTO workspace_state (workspace_id, updated_at, state) VALUES (1, 't', ?1)",
            params![state.to_string()],
        )
        .expect("state row");
        conn
    }

    fn graph() -> Value {
        json!({
            "version": sdrmm_wire::WORKSPACE_SNAPSHOT_VERSION,
            "graph": {
                "nodes": [
                    {"id": "ch", "kind": "channel", "data": {"channel_type": "nfm"}, "position": {"x": 10.0, "y": 20.0}},
                    {"id": "spk", "kind": "speaker", "position": {"x": 900.0, "y": 20.0}},
                    {"id": "rec", "kind": "audio_recorder", "data": {}, "position": {"x": 900.0, "y": 300.0}}
                ],
                "edges": [
                    {"from": {"node": "ch", "port": "audio"}, "to": {"node": "spk", "port": "audio"}},
                    {"from": {"node": "ch", "port": "audio"}, "to": {"node": "rec", "port": "audio"}}
                ]
            },
            "rack": {},
            "settings": {}
        })
    }

    fn state(type_id: &str, audio: &Value) -> Value {
        json!({
            "version": 2,
            "devices": [],
            "channels": [{
                "node": "ch",
                "settings": {
                    "frequency_hz": 145_500_000.0,
                    "params": {"type": type_id, "settings": {}},
                    "audio": audio
                }
            }],
            "trunks": []
        })
    }

    fn read(conn: &Connection) -> (WorkspaceSnapshot, WorkspaceState, Value) {
        let (snapshot, state): (String, String) = conn
            .query_row(
                "SELECT w.snapshot, s.state FROM workspaces w JOIN workspace_state s \
                 ON s.workspace_id = w.id WHERE w.id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("rows");
        (
            serde_json::from_str(&snapshot).expect("snapshot parses"),
            serde_json::from_str(&state).expect("state parses"),
            serde_json::from_str(&state).expect("state json"),
        )
    }

    #[test]
    fn a_channel_chain_becomes_an_audio_fx_node_on_its_audio_wires() {
        let conn = store_with(
            &graph(),
            &state(
                "nfm",
                &json!({"auto_notch": true, "blanker": {"enabled": true, "threshold": 7.0}}),
            ),
        );
        lift_audio_chains(&conn).expect("lift");
        let (snapshot, state, raw) = read(&conn);
        snapshot
            .graph
            .validate()
            .expect("the lifted graph is valid");
        let fx = snapshot
            .graph
            .nodes
            .iter()
            .find(|node| node.id == "audio_fx:ch")
            .expect("an audio FX node");
        let NodeBody::AudioFx(body) = &fx.body else {
            panic!("wrong kind {:?}", fx.body);
        };
        assert!(body.settings.auto_notch);
        let from_fx: Vec<_> = snapshot
            .graph
            .edges
            .iter()
            .filter(|edge| edge.from.node == "audio_fx:ch")
            .map(|edge| edge.to.node.as_str())
            .collect();
        assert_eq!(from_fx, ["spk", "rec"]);
        assert!(
            snapshot
                .graph
                .edges
                .iter()
                .any(|edge| edge.from.node == "ch" && edge.to.node == "audio_fx:ch")
        );
        assert!(state.channels[0].settings.blanker.enabled);
        assert!(raw["channels"][0]["settings"].get("audio").is_none());
    }

    #[test]
    fn the_old_am_default_adds_no_node() {
        let conn = store_with(&graph(), &state("am", &json!({"agc": "medium"})));
        lift_audio_chains(&conn).expect("lift");
        let (snapshot, _, raw) = read(&conn);
        assert_eq!(snapshot.graph.nodes.len(), 3);
        assert!(raw["channels"][0]["settings"].get("audio").is_none());
    }

    #[test]
    fn lifting_twice_changes_nothing_more() {
        let conn = store_with(&graph(), &state("nfm", &json!({"agc": "fast"})));
        lift_audio_chains(&conn).expect("lift");
        let once = read(&conn).0;
        lift_audio_chains(&conn).expect("lift again");
        assert_eq!(read(&conn).0, once);
    }
}
