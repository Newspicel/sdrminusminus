import type { Node, NodeProps } from "@xyflow/react";
import type { ComponentType } from "react";
import type { NodeKind, PatchNode } from "../../lib/types";
import type { FlowData } from "../Canvas";
import { ChannelFace } from "./ChannelFace";
import { DeviceFace } from "./DeviceFace";
import { DfFace } from "./DfFace";
import { DmrTrunkFace } from "./DmrTrunkFace";
import { EventFilterFace } from "./EventFilterFace";
import { EventOutputFace } from "./EventOutputFace";
import { GpsFace } from "./GpsFace";
import { NetworkExportFace } from "./NetworkExportFace";
import { CanvasSurface } from "./NodeShell";
import { PropagationFace } from "./PropagationFace";
import { RangeDopplerFace } from "./RangeDopplerFace";
import { ScopeFace } from "./ScopeFace";
import { SignalMapFace } from "./SignalMapFace";
import {
  AudioRecorderFace,
  BasebandRecorderFace,
  DecoderLogFace,
  ExportFace,
  HuntFace,
  MapFace,
  ReadoutFace,
  RecorderFace,
  ScannerFace,
  SpeakerFace,
  VideoFace,
} from "./SinkFaces";
import { TimeMachineFace } from "./TimeMachineFace";

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
  gps: mount(GpsFace),
  channel: mount(ChannelFace),
  event_output: mount(EventOutputFace),
  scope: mount(ScopeFace),
  speaker: mount(SpeakerFace),
  map: mount(MapFace),
  signal_map: mount(SignalMapFace),
  propagation: mount(PropagationFace),
  readout: mount(ReadoutFace),
  decoder_log: mount(DecoderLogFace),
  dmr_trunk: mount(DmrTrunkFace),
  event_filter: mount(EventFilterFace),
  video: mount(VideoFace),
  recorder: mount(RecorderFace),
  audio_recorder: mount(AudioRecorderFace),
  baseband_recorder: mount(BasebandRecorderFace),
  time_machine: mount(TimeMachineFace),
  network_export: mount(NetworkExportFace),
  export: mount(ExportFace),
  scanner: mount(ScannerFace),
  hunt: mount(HuntFace),
  df: mount(DfFace),
  passive_radar: mount(RangeDopplerFace),
};

export const FACES: Record<NodeKind, Face> = {
  device: DeviceFace,
  gps: GpsFace,
  channel: ChannelFace,
  event_output: EventOutputFace,
  scope: ScopeFace,
  speaker: SpeakerFace,
  map: MapFace,
  signal_map: SignalMapFace,
  propagation: PropagationFace,
  readout: ReadoutFace,
  decoder_log: DecoderLogFace,
  dmr_trunk: DmrTrunkFace,
  event_filter: EventFilterFace,
  video: VideoFace,
  recorder: RecorderFace,
  audio_recorder: AudioRecorderFace,
  baseband_recorder: BasebandRecorderFace,
  time_machine: TimeMachineFace,
  network_export: NetworkExportFace,
  export: ExportFace,
  scanner: ScannerFace,
  hunt: HuntFace,
  df: DfFace,
  passive_radar: RangeDopplerFace,
};
