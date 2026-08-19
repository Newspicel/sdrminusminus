import { DEFAULT_HUNT_SETTINGS } from "../components/hunt";
import { DEFAULT_HISTORY_SECONDS } from "../components/timeMachine";
import type { NodeBody, NodeKind, PositionSource } from "../lib/types";
import { DEFAULT_DF_PARAMS } from "./nodes/df";
import { DEFAULT_RADAR_PARAMS } from "./nodes/radar";

export interface NewNodeSeed {
  channelType?: string;
  source?: PositionSource;
}

const WITHOUT_DATA = new Set<NodeKind>([
  "scope",
  "speaker",
  "map",
  "readout",
  "decoder_log",
  "video",
  "recorder",
  "audio_recorder",
  "baseband_recorder",
  "export",
  "scanner",
]);

export function newNodeBody(kind: NodeKind, seed: NewNodeSeed = {}): NodeBody {
  switch (kind) {
    case "channel":
      return { kind, data: { channel_type: seed.channelType ?? "nfm", record_calls: false } };
    case "device":
      return { kind, data: {} };
    case "gps":
      return { kind, data: { source: seed.source ?? { type: "device" } } };
    case "signal_map":
      return { kind, data: { offset_hz: 0, bandwidth_hz: 12_500 } };
    case "propagation":
      return {
        kind,
        data: {
          half_life_minutes: 30,
          reflection_height_km: 300,
          show_paths: false,
          compare_forecast: true,
        },
      };
    case "dmr_trunk":
      return { kind, data: { protocol: "auto", record_calls: true } };
    case "event_filter":
      return {
        kind,
        data: {
          kinds: [],
          stations: [],
          talkgroups: [],
          radios: [],
          min_duration_ms: 0,
        },
      };
    case "network_export":
      return { kind, data: { transport: "udp", format: "cf32_le", address: "127.0.0.1:7355" } };
    case "hunt":
      return { kind, data: { settings: DEFAULT_HUNT_SETTINGS, clicks: true } };
    case "time_machine":
      return { kind, data: { history_seconds: DEFAULT_HISTORY_SECONDS } };
    case "event_output":
      return { kind, data: { target: { service: "webhook", url: "", format: "json" } } };
    case "df":
      return { kind, data: { settings: DEFAULT_DF_PARAMS } };
    case "passive_radar":
      return { kind, data: { settings: DEFAULT_RADAR_PARAMS } };
    default:
      return { kind } as NodeBody;
  }
}

export function carriesSettings(kind: NodeKind): boolean {
  return !WITHOUT_DATA.has(kind);
}
