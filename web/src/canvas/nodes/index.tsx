// The node registry: one entry per `NodeBody` kind (CANVAS §1). React Flow addresses components
// by the kind string, which is exactly what the stored patch holds — adding a node type means
// one wire variant and one entry here.
import type { Node, NodeProps } from "@xyflow/react";
import type { ComponentType } from "react";
import type { NodeKind, PatchNode } from "../../lib/types";
import type { FlowData } from "../Canvas";
import { ChannelFace } from "./ChannelFace";
import { DeviceFace } from "./DeviceFace";
import { CanvasSurface } from "./NodeShell";
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
 * A pinned face keeps its place on the canvas (CANVAS §5, amended): pinning adds a face to the
 * operate view, it does not take it out of the patch — a node that turned into a "pinned →"
 * placeholder left a hole where the operator had put an instrument, and made the patch a worse
 * picture of the station for having operated it.
 *
 * The rule it replaces existed to avoid two live surfaces for one instrument. Two of the three
 * costs are gone: the patch and the rack are alternate views, so only one is mounted at a time,
 * and scope faces now share one WebGL context across every view of them (CANVAS §7). Audio is
 * refcounted per (device set, channel) by the engine, so a second face is a second listener on
 * one stream. What is left is MapLibre, which takes a context per map instance — the reason a
 * split view showing both surfaces at once is not free, if one is ever built.
 */
function mount(Face: Face) {
  return function FaceNode({ data }: NodeProps<Node<FlowData>>) {
    return (
      <CanvasSurface>
        <Face node={data.node} />
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
