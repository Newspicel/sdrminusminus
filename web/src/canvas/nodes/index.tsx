// The node registry: one entry per `NodeBody` kind (CANVAS §1). React Flow addresses components
// by the kind string, which is exactly what the stored patch holds — adding a node type means
// one wire variant and one entry here.
import type { Node, NodeProps } from "@xyflow/react";
import type { ComponentType } from "react";
import type { NodeKind, PatchNode } from "../../lib/types";
import type { FlowData } from "../Canvas";
import { useStationContext } from "../context";
import { isPinned } from "../graph";
import { ChannelFace } from "./ChannelFace";
import { DeviceFace } from "./DeviceFace";
import { CanvasSurface, NodeShell } from "./NodeShell";
import { ScopeFace } from "./ScopeFace";
import {
  DecoderLogFace,
  ExportFace,
  MapFace,
  RecorderFace,
  ScannerFace,
  SpeakerFace,
} from "./SinkFaces";

type Face = ComponentType<{ node: PatchNode }>;

/**
 * One live surface per node (CANVAS §5): a pinned face renders in the rack, and its canvas node
 * collapses to a placeholder. Two mounts of one instrument would mean two WebGL contexts, two
 * map cameras and two audio subscriptions for one thing the operator sees once.
 */
function mount(Face: Face) {
  return function FaceNode({ data }: NodeProps<Node<FlowData>>) {
    const station = useStationContext();
    const node = data.node;
    if (isPinned(station.rack, node.id)) {
      return (
        <CanvasSurface>
          <NodeShell node={node} title={node.kind} category="display" live={false}>
            <p className="p-3 text-sm text-ink-dim">Pinned to the rack →</p>
          </NodeShell>
        </CanvasSurface>
      );
    }
    return (
      <CanvasSurface>
        <Face node={node} />
      </CanvasSurface>
    );
  };
}

export const NODE_TYPES: Record<NodeKind, ComponentType<NodeProps<Node<FlowData>>>> = {
  device: mount(DeviceFace),
  channel: mount(ChannelFace),
  scope: mount(ScopeFace),
  speaker: mount(SpeakerFace),
  map: mount(MapFace),
  decoder_log: mount(DecoderLogFace),
  recorder: mount(RecorderFace),
  export: mount(ExportFace),
  scanner: mount(ScannerFace),
};

/** The faces themselves, for the rack — which renders a face without a React Flow node around
 * it. */
export const FACES: Record<NodeKind, Face> = {
  device: DeviceFace,
  channel: ChannelFace,
  scope: ScopeFace,
  speaker: SpeakerFace,
  map: MapFace,
  decoder_log: DecoderLogFace,
  recorder: RecorderFace,
  export: ExportFace,
  scanner: ScannerFace,
};
