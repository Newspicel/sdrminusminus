use sdrmm_wire::{
    patch::{NodeBody, PatchNode},
    workspace::PatchApplyReport,
};

fn named(nodes: &[PatchNode], id: &str) -> String {
    let Some(node) = nodes.iter().find(|node| node.id == id) else {
        return id.to_owned();
    };
    if let Some(label) = &node.label {
        return label.clone();
    }
    match &node.body {
        NodeBody::Channel(channel) => channel.channel_type.to_uppercase(),
        _ => id.to_owned(),
    }
}

#[must_use]
pub fn apply_toasts(report: &PatchApplyReport, nodes: &[PatchNode]) -> Vec<String> {
    let refused = report
        .refused
        .iter()
        .map(|refusal| format!("{}: {}", named(nodes, &refusal.node), refusal.reason));
    let absent = report
        .absent
        .iter()
        .map(|node| format!("{}: radio not connected", named(nodes, node)));
    refused.chain(absent).collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        patch::{ChannelNode, DeviceNode, Position},
        workspace::PatchRefusal,
    };

    use super::*;

    fn node(id: &str, body: NodeBody, label: Option<&str>) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: label.map(str::to_owned),
        }
    }

    fn nodes() -> Vec<PatchNode> {
        vec![
            node(
                "device:a3ca5d1f",
                NodeBody::Device(DeviceNode::default()),
                Some("RTL-SDR 0"),
            ),
            node("device:bare", NodeBody::Device(DeviceNode::default()), None),
            node(
                "channel:1",
                NodeBody::Channel(ChannelNode {
                    channel_type: "nfm".to_owned(),
                    record_calls: false,
                    tuning_locked: false,
                }),
                None,
            ),
        ]
    }

    #[test]
    fn has_nothing_to_say_about_a_clean_apply() {
        assert!(apply_toasts(&PatchApplyReport::default(), &nodes()).is_empty());
    }

    #[test]
    fn names_a_node_by_its_label_a_channel_by_its_type_and_falls_back_to_the_id() {
        let report = PatchApplyReport {
            absent: vec!["device:a3ca5d1f".to_owned(), "device:bare".to_owned()],
            refused: vec![PatchRefusal {
                node: "channel:1".to_owned(),
                reason: "no room left".to_owned(),
            }],
            ..PatchApplyReport::default()
        };
        assert_eq!(
            apply_toasts(&report, &nodes()),
            [
                "NFM: no room left",
                "RTL-SDR 0: radio not connected",
                "device:bare: radio not connected"
            ]
        );
    }
}
