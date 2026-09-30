use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use sdrmm_wire::{DroppedNode, WorkspaceNotice, WorkspaceNoticeKind};
use serde_json::Value;

use super::{Store, StoreError, node_kind, now_rfc3339, remove_nodes};

pub(super) const RETIRED_KINDS: [&str; 5] = ["df", "combiner", "array", "passive_radar", "stitch"];

const BROKEN_VERSION: u64 = 3;

const UPGRADED_VERSION: u64 = 4;

const STATE_LISTS: [&str; 3] = ["devices", "channels", "trunks"];

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Broken {
    pub(crate) dropped: Vec<DroppedNode>,
    pub(crate) cleared_gps: Vec<String>,
}

impl Broken {
    pub(crate) fn notices(&self) -> Vec<WorkspaceNoticeKind> {
        let mut notices = Vec::new();
        if !self.dropped.is_empty() {
            notices.push(WorkspaceNoticeKind::DroppedNodes {
                nodes: self.dropped.clone(),
            });
        }
        if !self.cleared_gps.is_empty() {
            notices.push(WorkspaceNoticeKind::ClearedGps {
                nodes: self.cleared_gps.clone(),
            });
        }
        notices
    }

    fn dropped_ids(&self) -> HashSet<String> {
        self.dropped.iter().map(|node| node.id.clone()).collect()
    }
}

pub(crate) fn upgrade_snapshot(snapshot: &mut Value) -> Broken {
    if !is_broken_version(snapshot) {
        return Broken::default();
    }
    let mut broken = Broken::default();
    if let Some(nodes) = graph_nodes(snapshot) {
        broken.dropped = take_retired(nodes);
        broken.cleared_gps = clear_device_sources(nodes);
        give_triangulations_data(nodes);
    }
    remove_nodes(snapshot, &broken.dropped_ids());
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("version".to_owned(), Value::from(UPGRADED_VERSION));
    }
    broken
}

pub(crate) fn upgrade_export(export: &mut Value) -> Broken {
    let Some(snapshot) = export.get_mut("snapshot") else {
        return Broken::default();
    };
    let broken = upgrade_snapshot(snapshot);
    if let Some(state) = export.get_mut("state") {
        prune_state(state, &broken.dropped_ids());
    }
    broken
}

pub(super) fn break_old_snapshots(conn: &Connection) -> Result<(), StoreError> {
    let rows: Vec<(i64, Option<String>)> = conn
        .prepare("SELECT id, snapshot FROM workspaces ORDER BY id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1).ok())))?
        .collect::<Result<_, _>>()?;
    for (id, json) in rows {
        let Some(mut snapshot) = stored_layout(id, json.as_deref()) else {
            continue;
        };
        if !is_broken_version(&snapshot) {
            continue;
        }
        let broken = upgrade_snapshot(&mut snapshot);
        write_upgrade(conn, id, &snapshot, &broken)?;
    }
    Ok(())
}

fn stored_layout(id: i64, json: Option<&str>) -> Option<Value> {
    let Some(json) = json else {
        tracing::warn!(workspace = id, "stored layout is not text, left as it is");
        return None;
    };
    serde_json::from_str(json)
        .inspect_err(|err| {
            tracing::warn!(workspace = id, %err, "stored layout is not JSON, left as it is");
        })
        .ok()
}

pub(super) fn insert_notices(
    conn: &Connection,
    workspace: i64,
    notices: &[WorkspaceNoticeKind],
) -> Result<(), StoreError> {
    let at = now_rfc3339();
    for notice in notices {
        conn.execute(
            "INSERT INTO workspace_notices (workspace_id, created_at, notice) \
             VALUES (?1, ?2, ?3)",
            params![workspace, at, serde_json::to_string(notice)?],
        )?;
    }
    Ok(())
}

pub(super) fn read_notices(
    conn: &Connection,
    workspace: i64,
) -> Result<Vec<WorkspaceNotice>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, notice FROM workspace_notices \
         WHERE workspace_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![workspace], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (id, at, notice) = row?;
        Ok(WorkspaceNotice {
            id,
            at,
            notice: serde_json::from_str(&notice)?,
        })
    })
    .collect()
}

impl Store {
    pub fn dismiss_notice(&self, workspace: i64, notice: i64) -> Result<(), StoreError> {
        let conn = self.lock();
        let deleted = conn.execute(
            "DELETE FROM workspace_notices WHERE workspace_id = ?1 AND id = ?2",
            params![workspace, notice],
        )?;
        if deleted == 0 {
            return Err(StoreError::NoticeNotFound(notice));
        }
        Ok(())
    }
}

fn write_upgrade(
    conn: &Connection,
    id: i64,
    snapshot: &Value,
    broken: &Broken,
) -> Result<(), StoreError> {
    let nodes = snapshot
        .get("graph")
        .and_then(|graph| graph.get("nodes"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE workspaces SET snapshot = ?2, nodes = ?3, revision = revision + 1, \
         history_at = 0 WHERE id = ?1",
        params![id, serde_json::to_string(snapshot)?, nodes as i64],
    )?;
    tx.execute(
        "DELETE FROM workspace_history WHERE workspace_id = ?1",
        params![id],
    )?;
    prune_stored_state(&tx, id, &broken.dropped_ids())?;
    insert_notices(&tx, id, &broken.notices())?;
    tx.commit()?;
    Ok(())
}

fn prune_stored_state(
    conn: &Connection,
    workspace: i64,
    dropped: &HashSet<String>,
) -> Result<(), StoreError> {
    if dropped.is_empty() {
        return Ok(());
    }
    let stored: Option<String> = conn
        .query_row(
            "SELECT state FROM workspace_state WHERE workspace_id = ?1",
            params![workspace],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = stored else {
        return Ok(());
    };
    let mut state: Value = match serde_json::from_str(&json) {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(workspace, %err, "stored settings are not JSON, left as they are");
            return Ok(());
        }
    };
    if prune_state(&mut state, dropped) {
        conn.execute(
            "UPDATE workspace_state SET state = ?2 WHERE workspace_id = ?1",
            params![workspace, serde_json::to_string(&state)?],
        )?;
    }
    Ok(())
}

fn prune_state(state: &mut Value, dropped: &HashSet<String>) -> bool {
    let mut pruned = false;
    for list in STATE_LISTS {
        let Some(entries) = state.get_mut(list).and_then(Value::as_array_mut) else {
            continue;
        };
        let before = entries.len();
        entries.retain(|entry| {
            entry
                .get("node")
                .and_then(Value::as_str)
                .is_none_or(|node| !dropped.contains(node))
        });
        pruned |= entries.len() != before;
    }
    pruned
}

fn is_broken_version(snapshot: &Value) -> bool {
    snapshot.get("version").and_then(Value::as_u64) == Some(BROKEN_VERSION)
}

fn graph_nodes(snapshot: &mut Value) -> Option<&mut Vec<Value>> {
    snapshot.get_mut("graph")?.get_mut("nodes")?.as_array_mut()
}

fn text(node: &Value, key: &str) -> Option<String> {
    node.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn take_retired(nodes: &mut Vec<Value>) -> Vec<DroppedNode> {
    let mut dropped = Vec::new();
    nodes.retain(|node| {
        let Some(kind) = node_kind(node).filter(|kind| RETIRED_KINDS.contains(kind)) else {
            return true;
        };
        dropped.push(DroppedNode {
            id: text(node, "id").unwrap_or_default(),
            kind: kind.to_owned(),
            label: text(node, "label"),
        });
        false
    });
    dropped
}

fn clear_device_sources(nodes: &mut [Value]) -> Vec<String> {
    let mut cleared = Vec::new();
    for node in nodes
        .iter_mut()
        .filter(|node| node_kind(node) == Some("gps"))
    {
        let id = text(node, "id").unwrap_or_default();
        let Some(data) = node.get_mut("data").and_then(Value::as_object_mut) else {
            continue;
        };
        let from_device = data
            .get("source")
            .and_then(|source| source.get("type"))
            .and_then(Value::as_str)
            == Some("device");
        if from_device {
            data.remove("source");
            cleared.push(id);
        }
    }
    cleared
}

fn give_triangulations_data(nodes: &mut [Value]) {
    for node in nodes
        .iter_mut()
        .filter(|node| node_kind(node) == Some("triangulation"))
    {
        let Some(object) = node.as_object_mut() else {
            continue;
        };
        if object.get("data").is_none_or(Value::is_null) {
            object.insert("data".to_owned(), Value::Object(serde_json::Map::new()));
        }
    }
}

#[cfg(test)]
mod tests;
