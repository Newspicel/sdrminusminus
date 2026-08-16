use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::patch::{DeviceRef, NodeBody, PatchError, PatchGraph, PatchNode, Position, RackLayout};

pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 3;

pub const MAX_NAME_LEN: usize = 64;

const MERGE_GAP: f32 = 120.0;

const NATURAL_NODE_H: f32 = 380.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceSnapshot {
    pub version: u32,
    pub graph: PatchGraph,
    #[serde(default)]
    pub rack: RackLayout,
    #[serde(default)]
    pub settings: WorkspaceSettings,
}

pub const MAX_REGION_ID_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band_region: Option<String>,
    #[serde(default = "default_band_ruler")]
    pub band_ruler: bool,
}

const fn default_band_ruler() -> bool {
    true
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            band_region: None,
            band_ruler: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    Version(u32),
    Patch(PatchError),
    Name,
    Region,
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
            Self::Region => write!(
                f,
                "band region id must be 1..={MAX_REGION_ID_LEN} characters"
            ),
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
    #[must_use]
    pub fn new(graph: PatchGraph, rack: RackLayout) -> Self {
        Self {
            version: WORKSPACE_SNAPSHOT_VERSION,
            graph,
            rack,
            settings: WorkspaceSettings::default(),
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.version != WORKSPACE_SNAPSHOT_VERSION {
            return Err(WorkspaceError::Version(self.version));
        }
        self.graph.validate()?;
        self.rack.validate(&self.graph)?;
        if let Some(region) = &self.settings.band_region
            && (region.is_empty() || region.len() > MAX_REGION_ID_LEN)
        {
            return Err(WorkspaceError::Region);
        }
        Ok(())
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::new(PatchGraph::default(), RackLayout::default())
    }

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

    pub fn merge_patch(&mut self, patch: &PatchGraph, prefix: &str, device: Option<&DeviceRef>) {
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
            if !self.graph.edges.contains(&mapped_edge) {
                self.graph.edges.push(mapped_edge);
            }
        }
    }

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceInfo {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub nodes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceHistory {
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceDetail {
    #[serde(flatten)]
    pub info: WorkspaceInfo,
    pub snapshot: WorkspaceSnapshot,
    #[serde(default)]
    pub history: WorkspaceHistory,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkspaceRequest {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchBinding {
    pub node: String,
    pub device_set: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchApplyReport {
    pub bound: Vec<PatchBinding>,
    pub opened: u32,
    pub created: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<PatchRefusal>,
}

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

        let bare: WorkspaceSnapshot =
            serde_json::from_str(r#"{"version":3,"graph":{"nodes":[]}}"#).unwrap();
        assert!(bare.rack.slots.is_empty());
        assert!(bare.graph.edges.is_empty());
        assert_eq!(bare.settings.band_region, None);
        assert!(bare.settings.band_ruler);
    }

    #[test]
    fn validate_bounds_the_band_region_id() {
        let mut snap = WorkspaceSnapshot::starter();
        snap.settings.band_region = Some("itu1".to_owned());
        snap.validate().expect("a region id the server hands out");

        snap.settings.band_region = Some(String::new());
        assert_eq!(snap.validate(), Err(WorkspaceError::Region));

        snap.settings.band_region = Some("x".repeat(MAX_REGION_ID_LEN + 1));
        assert_eq!(snap.validate(), Err(WorkspaceError::Region));
    }

    #[test]
    fn validate_refuses_a_version_this_build_did_not_write() {
        let mut snap = WorkspaceSnapshot::starter();
        snap.version = 2;
        assert_eq!(snap.validate(), Err(WorkspaceError::Version(2)));
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
        assert_eq!(
            snap.graph
                .channels_of("template:airband:dev")
                .map(|(n, stream)| (n.id.as_str(), stream))
                .collect::<Vec<_>>(),
            vec![("template:airband:ch", 0)]
        );
    }

    #[test]
    fn re_applying_a_template_keeps_a_receiver_another_template_is_using() {
        let mut snap = WorkspaceSnapshot::starter();
        snap.merge_patch(&template(), "template:airband:", Some(&rtlsdr()));
        snap.merge_patch(&template(), "template:marine:", Some(&rtlsdr()));
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
