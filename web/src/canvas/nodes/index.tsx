import type { Node, NodeProps } from "@xyflow/react";
import type { ComponentType } from "react";
import type { NodeKind, PatchNode } from "../../lib/types";
import type { FlowData } from "../Canvas";
import { ChannelFace } from "./ChannelFace";
import { DeviceFace } from "./DeviceFace";
import { DmrTrunkFace } from "./DmrTrunkFace";
import { CanvasSurface } from "./NodeShell";
import { ScopeFace } from "./ScopeFace";
import {
  DecoderLogFace,
  ExportFace,
  MapFace,
  ReadoutFace,
  RecorderFace,
  ScannerFace,
  SpeakerFace,
  VideoFace,
} from "./SinkFaces";

type Face = ComponentType<{ node: PatchNode }>;

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
  readout: mount(ReadoutFace),
  decoder_log: mount(DecoderLogFace),
  dmr_trunk: mount(DmrTrunkFace),
  video: mount(VideoFace),
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
  readout: ReadoutFace,
  decoder_log: DecoderLogFace,
  dmr_trunk: DmrTrunkFace,
  video: VideoFace,
  recorder: RecorderFace,
  export: ExportFace,
  scanner: ScannerFace,
};
