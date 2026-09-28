import { DEFAULT_HISTORY_SECONDS } from "../components/timeMachine";
import catalog from "../generated/patch-catalog.json";
import type { NodeBody, NodeKind, PatchCatalog } from "../lib/types";

const CATALOG_BODIES = new Map<string, NodeBody>(
  (catalog as unknown as PatchCatalog).nodes.map((entry) => [entry.kind, entry.default_body]),
);

function catalogBody(kind: NodeKind): NodeBody {
  const body = CATALOG_BODIES.get(kind);
  if (body === undefined) {
    throw new Error(`the catalog has no ${kind}`);
  }
  return structuredClone(body);
}

export interface NewNodeSeed {
  channelType?: string;
}

const WITHOUT_DATA = new Set<NodeKind>([
  "scope",
  "baseband_scope",
  "speaker",
  "map",
  "readout",
  "decoder_log",
  "video",
  "export",
  "scanner",
]);

export function newNodeBody(kind: NodeKind, seed: NewNodeSeed = {}): NodeBody {
  switch (kind) {
    case "channel":
      return { kind, data: { channel_type: seed.channelType ?? "nfm", record_calls: false } };
    case "device":
      return { kind, data: {} };
    case "recording":
      return { kind, data: {} };
    case "signal_gen":
      return { kind, data: { running: true } };
    case "gps":
      return { kind, data: {} };
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
    case "spectrum_monitor":
      return { kind, data: { record_audio: true, min_confidence: 0.7 } };
    case "dmr_trunk":
      return { kind, data: { protocol: "auto", record_calls: true } };
    case "event_filter":
      return {
        kind,
        data: {
          mode: "keep",
          kinds: [],
          stations: [],
          talkgroups: [],
          radios: [],
          min_duration_ms: 0,
        },
      };
    case "audio_fx":
      return { kind, data: { settings: {} } };
    case "recorder":
    case "audio_recorder":
    case "baseband_recorder":
      return { kind, data: { recording: false } };
    case "network_export":
      return { kind, data: { transport: "udp", format: "cf32_le", address: "127.0.0.1:7355" } };
    case "hunt":
      return { kind, data: { clicks: true } };
    case "satellite":
      return { kind, data: {} };
    case "time_machine":
      return { kind, data: { history_seconds: DEFAULT_HISTORY_SECONDS } };
    case "event_output":
      return { kind, data: { target: { service: "webhook", url: "", format: "json" } } };
    case "triangulation":
      return { kind, data: {} };
    case "array":
    case "df":
    case "beamformer":
    case "passive_radar":
    case "stitch":
    case "spatial_spectrum":
    case "correlator":
    case "polarimeter":
      return catalogBody(kind);
    default:
      return { kind };
  }
}

export function carriesSettings(kind: NodeKind): boolean {
  return !WITHOUT_DATA.has(kind);
}

export function startsOnItsOwn(kind: NodeKind): boolean {
  return kind === "signal_gen";
}
