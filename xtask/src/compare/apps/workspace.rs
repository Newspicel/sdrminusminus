use sdrmm_wire::{
    ChannelNode, ChannelParams, ChannelSettings, NfmParams, NodeBody, NoiseBlankerSettings,
    PatchEdge, PatchGraph, PatchNode, PortRef, PortType, Position, RackLayout, RecordingNode,
    Squelch, WorkspaceSnapshot,
};

use super::signal::{self, CENTER_HZ, STEM};

const RECORDING: &str = "recording";
const SCOPE: &str = "scope";
const SPEAKER: &str = "speaker";
const COLUMN: f32 = 500.0;
const ROW: f32 = 140.0;

pub fn snapshot(receivers: usize) -> WorkspaceSnapshot {
    let mut nodes = vec![
        node(
            RECORDING,
            NodeBody::Recording(RecordingNode {
                recording: Some(STEM.to_owned()),
            }),
            0.0,
            0.0,
        ),
        node(SCOPE, NodeBody::Scope, COLUMN, 0.0),
        node(SPEAKER, NodeBody::Speaker, 2.0 * COLUMN, 0.0),
    ];
    let mut edges = vec![wire(RECORDING, SCOPE, PortType::Iq)];
    for index in 0..receivers {
        let id = channel(index);
        let row = 400.0 + index as f32 * ROW;
        nodes.push(node(
            &id,
            NodeBody::Channel(ChannelNode {
                channel_type: "nfm".to_owned(),
                tuning_locked: false,
            }),
            COLUMN,
            row,
        ));
        edges.push(wire(RECORDING, &id, PortType::Iq));
        edges.push(wire(&id, SPEAKER, PortType::Audio));
    }
    WorkspaceSnapshot::new(PatchGraph { nodes, edges }, RackLayout::default())
}

pub fn channel(index: usize) -> String {
    format!("nfm-{index}")
}

pub fn settings(index: usize) -> ChannelSettings {
    ChannelSettings {
        frequency_hz: CENTER_HZ + signal::offset_hz(index),
        squelch: Squelch::Off,
        params: ChannelParams::Nfm(NfmParams::default()),
        blanker: NoiseBlankerSettings::default(),
    }
}

fn node(id: &str, body: NodeBody, x: f32, y: f32) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x, y },
        size: None,
        label: None,
    }
}

fn wire(from: &str, to: &str, port: PortType) -> PatchEdge {
    let end = |node: &str| PortRef {
        node: node.to_owned(),
        port: port.as_str().to_owned(),
    };
    PatchEdge {
        from: end(from),
        to: end(to),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_is_valid_for_every_case() {
        for receivers in super::super::RECEIVERS {
            snapshot(receivers).validate().unwrap();
        }
    }

    #[test]
    fn every_channel_is_heard_on_the_speaker() {
        let graph = snapshot(4).graph;
        let heard = graph
            .edges
            .iter()
            .filter(|edge| edge.to.node == SPEAKER && edge.to.port == "audio")
            .count();
        assert_eq!(heard, 4);
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.from.node == RECORDING && edge.to.node == SCOPE)
        );
    }

    #[test]
    fn channels_sit_on_their_own_carrier() {
        assert_eq!(settings(1).frequency_hz, CENTER_HZ + signal::offset_hz(1));
        assert_eq!(settings(1).params.type_id(), "nfm");
    }
}
