//! Workspaces — the persisted station (PLAN §10, CANVAS §4). A workspace holds one patch graph
//! and one rack layout, and exactly one workspace is active server-side, so every client that
//! opens the station sees the same setup.
//!
//! The graph is *our* model, not the canvas library's serialization: templates author stations in
//! Rust (CANVAS §8 phase ④), the server is the source of truth for type definitions (PLAN §2),
//! and a React Flow major must not invalidate stored workspaces. The shape lives in
//! [`crate::patch`]; this module is the stored row around it.
//!
//! What a stored node may name changed at M7 and the reason did not: engine ids are allocated per
//! run and reused, so a node names a *device* by durable identity ([`crate::DeviceRef`]) and
//! never a device set. The M6 rule that a panel could name no radio at all is retired — spatial
//! identity is the point of the canvas (PLAN §18) — but nothing per-run is stored to buy it.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::patch::{DeviceRef, NodeBody, PatchError, PatchGraph, PatchNode, Position, RackLayout};

/// Shape version of a stored [`WorkspaceSnapshot`]. A build refuses to *write back* a snapshot it
/// did not itself produce: a station is re-persisted on every arrangement gesture, so a downgrade
/// would silently rewrite a newer one with whatever this build understood of it.
///
/// Version 2 is the canvas (M7). Version 1 was the tabs-and-dockview tree; stored v1 rows do not
/// migrate (CANVAS §8 phase ⑤: a clean reset, recorded rather than converted, because the new
/// model cannot express a dock layout and nobody would want it to).
pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 2;

/// Bound on every user-visible name here: a workspace is picked by name in the switcher and a
/// node by its label on the canvas, so an unbounded string is a layout bug, not a feature.
pub const MAX_NAME_LEN: usize = 64;

/// Vertical gap between an existing station and a patch merged under it, in canvas units.
const MERGE_GAP: f32 = 120.0;

/// The stored body of a workspace (PLAN §11: one JSON snapshot per row, like presets — written
/// atomically, read whole, never queried by inner field).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceSnapshot {
    /// [`WORKSPACE_SNAPSHOT_VERSION`] at the time of writing.
    pub version: u32,
    pub graph: PatchGraph,
    /// Faces pinned to the operate view. May be empty — the canvas alone is a complete UI
    /// (CANVAS §5).
    #[serde(default)]
    pub rack: RackLayout,
}

/// Why a snapshot was refused. Structural only — the checks are pure, so they run in `wire` and
/// the server has one rejection point instead of scattered guards. `Display` is written out
/// rather than derived because this crate carries no error-derive dependency (PLAN §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    Version(u32),
    Patch(PatchError),
    Name,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version(v) => write!(
                f,
                "unsupported workspace snapshot version {v} (this build writes \
                 {WORKSPACE_SNAPSHOT_VERSION})"
            ),
            Self::Patch(err) => write!(f, "{err}"),
            Self::Name => write!(f, "name must be 1..={MAX_NAME_LEN} characters"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<PatchError> for WorkspaceError {
    fn from(err: PatchError) -> Self {
        Self::Patch(err)
    }
}

impl WorkspaceSnapshot {
    /// A snapshot holding `graph` and `rack` at this build's version.
    #[must_use]
    pub fn new(graph: PatchGraph, rack: RackLayout) -> Self {
        Self {
            version: WORKSPACE_SNAPSHOT_VERSION,
            graph,
            rack,
        }
    }

    /// Structural bounds a stored station must satisfy. Client-built graphs come from a canvas
    /// the same rules already policed at drag time, so a malformed one is a client bug or a
    /// corrupt row — either way it is refused at the API edge rather than half-applied.
    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.version != WORKSPACE_SNAPSHOT_VERSION {
            return Err(WorkspaceError::Version(self.version));
        }
        self.graph.validate()?;
        self.rack.validate(&self.graph)?;
        Ok(())
    }

    /// The station a fresh install opens on: one empty receiver node feeding a scope, with a
    /// speaker waiting for a channel. Empty rather than pre-populated because the receiver node
    /// *is* the "open a radio" invitation — picking a device in it is the first gesture.
    #[must_use]
    pub fn station_default() -> Self {
        let node = |id: &str, body: NodeBody, x: f32, y: f32| PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x, y },
            size: None,
            label: None,
        };
        let graph = PatchGraph {
            nodes: vec![
                node(
                    "device",
                    NodeBody::Device(crate::patch::DeviceNode::default()),
                    0.0,
                    0.0,
                ),
                node("scope", NodeBody::Scope, 420.0, 0.0),
                node("speaker", NodeBody::Speaker, 420.0, 380.0),
            ],
            edges: vec![crate::patch::PatchEdge {
                from: crate::patch::PortRef {
                    node: "device".to_owned(),
                    port: "iq".to_owned(),
                },
                to: crate::patch::PortRef {
                    node: "scope".to_owned(),
                    port: "iq".to_owned(),
                },
            }],
        };
        Self::new(graph, RackLayout::default())
    }

    /// Merge an authored patch (a template, CANVAS §8 phase ④) into this station.
    ///
    /// Node ids are namespaced by `prefix` and the prefix's previous nodes are removed first, so
    /// applying a template twice replaces its own block instead of stacking copies — the same
    /// contract M6's `upsert_tab` had. A device node in the patch binds to `device`: if the
    /// station already has a node for that radio the patch wires into *it* rather than drawing a
    /// second box for one receiver.
    pub fn merge_patch(&mut self, patch: &PatchGraph, prefix: &str, device: Option<&DeviceRef>) {
        self.remove_prefixed(prefix);

        let existing_device = device.and_then(|want| {
            self.graph
                .device_nodes()
                .find(|node| match &node.body {
                    NodeBody::Device(d) => d.device.as_ref() == Some(want),
                    _ => false,
                })
                .map(|node| node.id.clone())
        });
        let offset = self.merge_offset();

        let mut mapped: Vec<(String, String)> = Vec::with_capacity(patch.nodes.len());
        for node in &patch.nodes {
            let is_device = matches!(node.body, NodeBody::Device(_));
            if is_device && let Some(reuse) = &existing_device {
                mapped.push((node.id.clone(), reuse.clone()));
                continue;
            }
            let id = format!("{prefix}{}", node.id);
            mapped.push((node.id.clone(), id.clone()));
            let body = match (&node.body, device) {
                // An authored patch leaves its receiver unbound; applying it to a radio is what
                // names one.
                (NodeBody::Device(d), Some(want)) if d.device.is_none() => {
                    NodeBody::Device(crate::patch::DeviceNode {
                        device: Some(want.clone()),
                    })
                }
                (body, _) => body.clone(),
            };
            self.graph.nodes.push(PatchNode {
                id,
                body,
                position: Position {
                    x: node.position.x,
                    y: node.position.y + offset,
                },
                size: node.size,
                label: node.label.clone(),
            });
        }

        let remap = |id: &str| -> Option<String> {
            mapped
                .iter()
                .find(|(from, _)| from == id)
                .map(|(_, to)| to.clone())
        };
        for edge in &patch.edges {
            let (Some(from), Some(to)) = (remap(&edge.from.node), remap(&edge.to.node)) else {
                continue;
            };
            let mapped_edge = crate::patch::PatchEdge {
                from: crate::patch::PortRef {
                    node: from,
                    port: edge.from.port.clone(),
                },
                to: crate::patch::PortRef {
                    node: to,
                    port: edge.to.port.clone(),
                },
            };
            // Reusing a device node can reproduce a wire the station already has.
            if !self.graph.edges.contains(&mapped_edge) {
                self.graph.edges.push(mapped_edge);
            }
        }
    }

    /// Drop every node this prefix owns, and everything that referenced them.
    fn remove_prefixed(&mut self, prefix: &str) {
        self.graph.nodes.retain(|node| !node.id.starts_with(prefix));
        self.graph.edges.retain(|edge| {
            !edge.from.node.starts_with(prefix) && !edge.to.node.starts_with(prefix)
        });
        self.rack
            .slots
            .retain(|slot| !slot.node.starts_with(prefix));
    }

    /// Where a merged block starts: under everything already drawn, so a template never lands on
    /// top of the station it is being added to.
    fn merge_offset(&self) -> f32 {
        self.graph
            .nodes
            .iter()
            .map(|node| node.position.y + node.size.map_or(0.0, |s| s.h))
            .fold(f32::NEG_INFINITY, f32::max)
            .max(0.0)
            + MERGE_GAP
    }
}

/// `GET /api/workspaces` list entry — the projection a switcher needs, without the station.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceInfo {
    pub id: i64,
    pub name: String,
    /// RFC3339 UTC.
    pub created_at: String,
    /// RFC3339 UTC.
    pub updated_at: String,
    /// Bumped on every stored change. An update carrying a stale revision is refused rather than
    /// silently overwriting another client's arrangement.
    pub revision: u64,
    /// Node count, denormalized so the switcher can describe a workspace without parsing its
    /// graph.
    pub nodes: u32,
}

/// `GET /api/workspaces`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceInfo>,
    /// The active workspace, or `None` when the last one was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<i64>,
}

/// `GET /api/workspaces/{id}` — the row plus its station.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceDetail {
    #[serde(flatten)]
    pub info: WorkspaceInfo,
    pub snapshot: WorkspaceSnapshot,
}

/// `POST /api/workspaces`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    /// Station to start from; omitted means [`WorkspaceSnapshot::station_default`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

/// `PUT /api/workspaces/{id}` — rename, re-patch, or both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkspaceRequest {
    /// The revision the client last saw. A mismatch is a `409`, never a silent overwrite.
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

/// One device node now driving an engine device set (CANVAS §3). Bindings are recomputed per run
/// and never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchBinding {
    pub node: String,
    pub device_set: u32,
}

/// `POST /api/workspaces/{id}/apply` — what applying the station did.
///
/// Apply is additive and idempotent: it opens the radios the graph names and adds the channels it
/// draws, and never closes or deletes anything. Removing a node is a gesture with its own
/// endpoint; a reconciler that also deleted would turn "this workspace has fewer nodes" into
/// "close that operator's radio".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchApplyReport {
    /// Every device node that now has a running device set.
    pub bound: Vec<PatchBinding>,
    /// Device sets opened by this call.
    pub opened: u32,
    /// Channels created by this call.
    pub created: u32,
    /// Device nodes whose radio is not attached; they render disconnected (CANVAS §3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
    /// Nodes apply could not satisfy, with the reason — a wideband channel on a device running
    /// at the wrong rate is the common one (PLAN §18). Reported, never silently skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<PatchRefusal>,
}

/// One node apply could not satisfy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchRefusal {
    pub node: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::{ChannelNode, DeviceNode, PatchEdge, PortRef, RackCell, RackSlot};

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn wire(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.to_owned(),
                port: from.1.to_owned(),
            },
            to: PortRef {
                node: to.0.to_owned(),
                port: to.1.to_owned(),
            },
        }
    }

    fn rtlsdr() -> DeviceRef {
        DeviceRef {
            backend: "rtlsdr".to_owned(),
            serial: Some("00000001".to_owned()),
            key: None,
        }
    }

    /// An airband-style template: a receiver, two channels and a speaker.
    fn template() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node("dev", NodeBody::Device(DeviceNode::default())),
                node(
                    "ch",
                    NodeBody::Channel(ChannelNode {
                        channel_type: "am".to_owned(),
                    }),
                ),
                node("spk", NodeBody::Speaker),
            ],
            edges: vec![
                wire(("dev", "iq"), ("ch", "iq")),
                wire(("ch", "audio"), ("spk", "audio")),
            ],
        }
    }

    #[test]
    fn station_default_validates_and_is_a_receiver_with_a_scope() {
        let snap = WorkspaceSnapshot::station_default();
        snap.validate().expect("the default is valid");
        assert_eq!(snap.version, WORKSPACE_SNAPSHOT_VERSION);
        assert_eq!(snap.graph.nodes.len(), 3);
        assert!(snap.rack.slots.is_empty());
        assert_eq!(snap.graph.device_nodes().count(), 1);
        assert_eq!(
            snap.graph.targets_of("device", "iq").collect::<Vec<_>>(),
            vec!["scope"]
        );
    }

    #[test]
    fn snapshot_roundtrips_through_json_and_omits_an_empty_rack_body() {
        let snap = WorkspaceSnapshot::station_default();
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["version"], WORKSPACE_SNAPSHOT_VERSION);
        assert_eq!(json["graph"]["nodes"][0]["kind"], "device");
        assert!(
            json["graph"]["nodes"][0]["data"].get("device").is_none(),
            "an unbound receiver names no radio"
        );
        assert_eq!(json["rack"]["slots"].as_array().map(Vec::len), Some(0));
        let back: WorkspaceSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back, snap);

        // A snapshot from a peer that omits the rack entirely still reads.
        let bare: WorkspaceSnapshot =
            serde_json::from_str(r#"{"version":2,"graph":{"nodes":[]}}"#).unwrap();
        assert!(bare.rack.slots.is_empty());
        assert!(bare.graph.edges.is_empty());
    }

    #[test]
    fn validate_refuses_a_version_this_build_did_not_write() {
        let mut snap = WorkspaceSnapshot::station_default();
        snap.version = 1;
        assert_eq!(snap.validate(), Err(WorkspaceError::Version(1)));
    }

    #[test]
    fn validate_surfaces_the_graph_and_rack_reasons() {
        let mut broken = WorkspaceSnapshot::station_default();
        broken
            .graph
            .edges
            .push(wire(("device", "iq"), ("ghost", "iq")));
        assert_eq!(
            broken.validate(),
            Err(WorkspaceError::Patch(PatchError::UnknownNode(
                "ghost".to_owned()
            )))
        );

        let mut pinned_ghost = WorkspaceSnapshot::station_default();
        pinned_ghost.rack.slots.push(RackSlot {
            node: "ghost".to_owned(),
            cell: RackCell {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
        });
        assert!(matches!(
            pinned_ghost.validate(),
            Err(WorkspaceError::Patch(PatchError::UnknownNode(_)))
        ));
    }

    #[test]
    fn merging_a_template_namespaces_it_binds_the_radio_and_lands_below() {
        let mut snap = WorkspaceSnapshot::station_default();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.validate().expect("still valid after a merge");

        // The station's own receiver was unbound, so the template drew its own — bound.
        let ids: Vec<&str> = snap.graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"template:airband:ch"));
        assert!(ids.contains(&"template:airband:dev"));
        let merged = snap.graph.node("template:airband:dev").unwrap();
        assert_eq!(
            merged.body,
            NodeBody::Device(DeviceNode {
                device: Some(rtlsdr())
            })
        );
        assert!(
            merged.position.y >= MERGE_GAP,
            "a merged patch lands under the station, not on it"
        );
        assert_eq!(
            snap.graph
                .channels_of("template:airband:dev")
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["template:airband:ch"]
        );
    }

    #[test]
    fn re_applying_a_template_replaces_its_own_block() {
        let mut snap = WorkspaceSnapshot::station_default();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        let after_first = snap.graph.nodes.len();
        let edges_first = snap.graph.edges.len();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        assert_eq!(snap.graph.nodes.len(), after_first, "re-apply must replace");
        assert_eq!(snap.graph.edges.len(), edges_first);
        snap.validate().expect("valid after a re-apply");
    }

    /// One radio, one box: a template applied to a receiver the station already draws wires into
    /// that node instead of adding a second one for the same hardware.
    #[test]
    fn merging_reuses_a_device_node_that_already_names_the_radio() {
        let mut snap = WorkspaceSnapshot::station_default();
        let NodeBody::Device(device) = &mut snap.graph.nodes[0].body else {
            unreachable!("the default station opens with a receiver node")
        };
        device.device = Some(rtlsdr());

        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.validate().expect("valid");
        assert_eq!(snap.graph.device_nodes().count(), 1, "one radio, one node");
        assert!(snap.graph.node("template:airband:dev").is_none());
        assert_eq!(
            snap.graph
                .channels_of("device")
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["template:airband:ch"]
        );
    }

    #[test]
    fn apply_report_omits_the_quiet_fields() {
        let json = serde_json::to_value(PatchApplyReport {
            bound: vec![PatchBinding {
                node: "device".to_owned(),
                device_set: 1,
            }],
            opened: 1,
            created: 2,
            ..PatchApplyReport::default()
        })
        .unwrap();
        assert_eq!(json["bound"][0]["device_set"], 1);
        assert_eq!(json["opened"], 1);
        assert!(json.get("absent").is_none());
        assert!(json.get("refused").is_none());
    }
}
