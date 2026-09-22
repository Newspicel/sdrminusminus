import type { Node, NodeProps } from "@xyflow/react";
import type { ComponentType, CSSProperties } from "react";
import type { NodeKind, PatchNode } from "../../lib/types";
import type { FlowData } from "../Canvas";
import { NODE_SIZE } from "../graph";
import { ArrayFace } from "./ArrayFace";
import { BasebandScopeFace } from "./BasebandScopeFace";
import { ChannelFace } from "./ChannelFace";
import { CombinerFace } from "./CombinerFace";
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
import { RecordingFace } from "./RecordingFace";
import { SatelliteFace } from "./SatelliteFace";
import { ScopeFace } from "./ScopeFace";
import { SignalGenFace } from "./SignalGenFace";
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
import { SpectrumMonitorFace } from "./SpectrumMonitorFace";
import { TimeMachineFace } from "./TimeMachineFace";
import { TriangulationFace } from "./TriangulationFace";

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
  recording: mount(RecordingFace),
  signal_gen: mount(SignalGenFace),
  array: mount(ArrayFace),
  gps: mount(GpsFace),
  channel: mount(ChannelFace),
  event_output: mount(EventOutputFace),
  scope: mount(ScopeFace),
  baseband_scope: mount(BasebandScopeFace),
  speaker: mount(SpeakerFace),
  map: mount(MapFace),
  signal_map: mount(SignalMapFace),
  propagation: mount(PropagationFace),
  readout: mount(ReadoutFace),
  decoder_log: mount(DecoderLogFace),
  dmr_trunk: mount(DmrTrunkFace),
  spectrum_monitor: mount(SpectrumMonitorFace),
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
  satellite: mount(SatelliteFace),
  df: mount(DfFace),
  passive_radar: mount(RangeDopplerFace),
  combiner: mount(CombinerFace),
  triangulation: mount(TriangulationFace),
};

export const FACES: Record<NodeKind, Face> = {
  device: DeviceFace,
  recording: RecordingFace,
  signal_gen: SignalGenFace,
  array: ArrayFace,
  gps: GpsFace,
  channel: ChannelFace,
  event_output: EventOutputFace,
  scope: ScopeFace,
  baseband_scope: BasebandScopeFace,
  speaker: SpeakerFace,
  map: MapFace,
  signal_map: SignalMapFace,
  propagation: PropagationFace,
  readout: ReadoutFace,
  decoder_log: DecoderLogFace,
  dmr_trunk: DmrTrunkFace,
  spectrum_monitor: SpectrumMonitorFace,
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
  satellite: SatelliteFace,
  df: DfFace,
  passive_radar: RangeDopplerFace,
  combiner: CombinerFace,
  triangulation: TriangulationFace,
};

export function faceSize(node: PatchNode): CSSProperties {
  const size = NODE_SIZE[node.kind];
  return { width: size.h === undefined ? size.w : "100%", height: "100%" };
}
