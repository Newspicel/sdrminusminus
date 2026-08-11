//! Workspaces — the persisted workspace (PLAN §10, CANVAS §4). A workspace holds one patch graph
//! and one rack layout, and exactly one workspace is active server-side, so every client that
//! opens the workspace sees the same setup.
//!
//! The graph is *our* model, not the canvas library's serialization: templates author workspaces in
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
/// did not itself produce: a workspace is re-persisted on every arrangement gesture, so a downgrade
/// would silently rewrite a newer one with whatever this build understood of it.
///
/// Version 2 is the canvas (M7). Version 1 was the tabs-and-dockview tree; stored v1 rows do not
/// migrate (CANVAS §8 phase ⑤: a clean reset, recorded rather than converted, because the new
/// model cannot express a dock layout and nobody would want it to).
pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 2;

/// Bound on every user-visible name here: a workspace is picked by name in the switcher and a
/// node by its label on the canvas, so an unbounded string is a layout bug, not a feature.
pub const MAX_NAME_LEN: usize = 64;

/// Vertical gap between an existing workspace and a patch merged under it, in canvas units.
const MERGE_GAP: f32 = 120.0;

/// Height to assume for a node that has never been resized. `size` is `None` until the operator
/// drags a corner, so without this the merge offset would measure to the *top* of the lowest node
/// and drop the new block on top of it. A face is as tall as its instrument now
/// (`web/src/canvas/graph.ts`, `NODE_SIZE`), so this is a generous stand-in for the tallest of
/// them — erring high only opens a gap, erring low overlaps two workspaces.
const NATURAL_NODE_H: f32 = 380.0;

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

    /// Structural bounds a stored workspace must satisfy. Client-built graphs come from a canvas
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

    /// A workspace with nothing on it. What `POST /api/workspaces` creates unless the caller
    /// sends a snapshot: a new workspace is a clean bench, and an operator who wanted a device
    /// and a scope on it would rather draw them than delete someone else's guess.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(PatchGraph::default(), RackLayout::default())
    }

    /// The workspace a fresh install opens on: one empty device node feeding a scope, with a
    /// speaker waiting for a channel. Empty rather than pre-populated because the device node
    /// *is* the "open a radio" invitation — picking a device in it is the first gesture. Only the
    /// seeded first workspace starts here; every later one starts [`empty`](Self::empty).
    #[must_use]
    pub fn starter() -> Self {
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

    /// Merge an authored patch (a template, CANVAS §8 phase ④) into this workspace.
    ///
    /// Node ids are namespaced by `prefix` and the prefix's previous nodes are removed first, so
    /// applying a template twice replaces its own block instead of stacking copies — the same
    /// contract M6's `upsert_tab` had. A device node in the patch binds to `device`: if the
    /// workspace already has a node for that radio the patch wires into *it* rather than drawing a
    /// second box for one device.
    pub fn merge_patch(&mut self, patch: &PatchGraph, prefix: &str, device: Option<&DeviceRef>) {
        // Where the prefix's nodes were, so a re-apply puts them back in the same place. Node
        // order is binding order (CANVAS §3), so appending them instead would renumber which
        // face drives which channel whenever a template is re-applied over another one's.
        let at = self.remove_prefixed(prefix);

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

        let base = at.unwrap_or(self.graph.nodes.len());
        let mut mapped: Vec<(String, String)> = Vec::with_capacity(patch.nodes.len());
        let mut added = 0;
        for node in &patch.nodes {
            let is_device = matches!(node.body, NodeBody::Device(_));
            if is_device && let Some(reuse) = &existing_device {
                mapped.push((node.id.clone(), reuse.clone()));
                continue;
            }
            let id = format!("{prefix}{}", node.id);
            mapped.push((node.id.clone(), id.clone()));
            let body = match (&node.body, device) {
                // An authored patch leaves its device unbound; applying it to a radio is what
                // names one.
                (NodeBody::Device(d), Some(want)) if d.device.is_none() => {
                    NodeBody::Device(crate::patch::DeviceNode {
                        device: Some(want.clone()),
                    })
                }
                (body, _) => body.clone(),
            };
            let placed = PatchNode {
                id,
                body,
                position: Position {
                    x: node.position.x,
                    y: node.position.y + offset,
                },
                size: node.size,
                label: node.label.clone(),
            };
            self.graph
                .nodes
                .insert((base + added).min(self.graph.nodes.len()), placed);
            added += 1;
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
            // Reusing a device node can reproduce a wire the workspace already has.
            if !self.graph.edges.contains(&mapped_edge) {
                self.graph.edges.push(mapped_edge);
            }
        }
    }

    /// Drop every node this prefix owns, and everything that referenced them.
    ///
    /// One exception: a *device* node this prefix created may since have become the radio
    /// another template's channels hang off, and taking it away would unwire them. It is kept,
    /// and the merge that follows finds it again by its [`DeviceRef`] — so the re-applied
    /// template wires into the same box instead of drawing a second one for one antenna.
    ///
    /// Returns where the prefix's first node was, so the caller can put its replacement back
    /// there.
    fn remove_prefixed(&mut self, prefix: &str) -> Option<usize> {
        let shared: Vec<String> = self
            .graph
            .nodes
            .iter()
            .filter(|node| node.id.starts_with(prefix) && matches!(node.body, NodeBody::Device(_)))
            .filter(|node| {
                self.graph.edges.iter().any(|edge| {
                    (edge.from.node == node.id && !edge.to.node.starts_with(prefix))
                        || (edge.to.node == node.id && !edge.from.node.starts_with(prefix))
                })
            })
            .map(|node| node.id.clone())
            .collect();
        let dropped = |id: &str| id.starts_with(prefix) && !shared.iter().any(|kept| kept == id);
        let at = self.graph.nodes.iter().position(|node| dropped(&node.id));
        self.graph.nodes.retain(|node| !dropped(&node.id));
        self.graph
            .edges
            .retain(|edge| !dropped(&edge.from.node) && !dropped(&edge.to.node));
        self.rack.slots.retain(|slot| !dropped(&slot.node));
        at
    }

    /// Where a merged block starts: under everything already drawn, so a template never lands on
    /// top of the workspace it is being added to.
    fn merge_offset(&self) -> f32 {
        self.graph
            .nodes
            .iter()
            .map(|node| node.position.y + node.size.map_or(NATURAL_NODE_H, |s| s.h))
            .fold(f32::NEG_INFINITY, f32::max)
            .max(0.0)
            + MERGE_GAP
    }
}

/// `GET /api/workspaces` list entry — the projection a switcher needs, without the workspace.
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

/// `GET /api/workspaces/{id}` — the row plus its workspace.
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
    /// Workspace to start from; omitted means [`WorkspaceSnapshot::empty`] — a new workspace is a
    /// clean bench. Only the first workspace a fresh install seeds opens on a starter workspace.
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

/// `POST /api/workspaces/{id}/apply` — what applying the workspace did.
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

    /// An airband-style template: a device, two channels and a speaker.
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
    fn starter_validates_and_is_a_receiver_with_a_scope() {
        let snap = WorkspaceSnapshot::starter();
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
        let snap = WorkspaceSnapshot::starter();
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["version"], WORKSPACE_SNAPSHOT_VERSION);
        assert_eq!(json["graph"]["nodes"][0]["kind"], "device");
        assert!(
            json["graph"]["nodes"][0]["data"].get("device").is_none(),
            "an unbound device names no radio"
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
        let mut snap = WorkspaceSnapshot::starter();
        snap.version = 1;
        assert_eq!(snap.validate(), Err(WorkspaceError::Version(1)));
    }

    #[test]
    fn validate_surfaces_the_graph_and_rack_reasons() {
        let mut broken = WorkspaceSnapshot::starter();
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

        let mut pinned_ghost = WorkspaceSnapshot::starter();
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
        let mut snap = WorkspaceSnapshot::starter();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.validate().expect("still valid after a merge");

        // The workspace's own device was unbound, so the template drew its own — bound.
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
            "a merged patch lands under the workspace, not on it"
        );
        // Templates name the bare `iq`, which is and stays stream 0.
        assert_eq!(
            snap.graph
                .channels_of("template:airband:dev")
                .map(|(n, stream)| (n.id.as_str(), stream))
                .collect::<Vec<_>>(),
            vec![("template:airband:ch", 0)]
        );
    }

    /// A second template wires into the device the first one drew. Re-applying the first must
    /// not take that device — and the channels hanging off it — away with its own block.
    #[test]
    fn re_applying_a_template_keeps_a_receiver_another_template_is_using() {
        let mut snap = WorkspaceSnapshot::starter();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.merge_patch(&template(), "template:marine:", Some(&rtlsdr()));
        // The second template reused the first's device rather than drawing its own.
        assert!(snap.graph.node("template:marine:dev").is_none());
        assert_eq!(
            snap.graph
                .channels_of("template:airband:dev")
                .map(|(n, _)| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["template:airband:ch", "template:marine:ch"]
        );

        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.validate().expect("valid after the re-apply");
        assert!(
            snap.graph.node("template:airband:dev").is_some(),
            "the shared device survives its own template's re-apply"
        );
        assert_eq!(
            snap.graph
                .channels_of("template:airband:dev")
                .map(|(n, _)| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["template:airband:ch", "template:marine:ch"],
            "the other template's channel keeps its radio"
        );
    }

    /// A node that was never resized still occupies space; measuring to its top would drop the
    /// merged block on top of it.
    #[test]
    fn merge_offset_clears_a_node_that_has_never_been_resized() {
        let mut snap = WorkspaceSnapshot::starter();
        let lowest = snap
            .graph
            .nodes
            .iter()
            .map(|node| node.position.y)
            .fold(f32::NEG_INFINITY, f32::max);
        snap.merge_patch(&template(), "template:airband:", None);
        let merged = snap
            .graph
            .nodes
            .iter()
            .filter(|node| node.id.starts_with("template:"))
            .map(|node| node.position.y)
            .fold(f32::INFINITY, f32::min);
        assert!(
            merged >= lowest + NATURAL_NODE_H,
            "merged at {merged}, which overlaps a face at {lowest}"
        );
    }

    #[test]
    fn re_applying_a_template_replaces_its_own_block() {
        let mut snap = WorkspaceSnapshot::starter();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        let after_first = snap.graph.nodes.len();
        let edges_first = snap.graph.edges.len();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        assert_eq!(snap.graph.nodes.len(), after_first, "re-apply must replace");
        assert_eq!(snap.graph.edges.len(), edges_first);
        snap.validate().expect("valid after a re-apply");
    }

    /// One radio, one box: a template applied to a device the workspace already draws wires into
    /// that node instead of adding a second one for the same hardware.
    #[test]
    fn merging_reuses_a_device_node_that_already_names_the_radio() {
        let mut snap = WorkspaceSnapshot::starter();
        let NodeBody::Device(device) = &mut snap.graph.nodes[0].body else {
            unreachable!("the default workspace opens with a device node")
        };
        device.device = Some(rtlsdr());

        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.validate().expect("valid");
        assert_eq!(snap.graph.device_nodes().count(), 1, "one radio, one node");
        assert!(snap.graph.node("template:airband:dev").is_none());
        assert_eq!(
            snap.graph
                .channels_of("device")
                .map(|(n, _)| n.id.as_str())
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
