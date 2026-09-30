import type {
  ArrayLaneStatus,
  ArrayStatus,
  Capabilities,
  DecodedRecord,
  DeviceSet,
  DfBearing,
  PatchNode,
  RadarDetection,
  RadarUpdate,
} from "../lib/types";

export function capabilities(overrides: Partial<Capabilities> = {}): Capabilities {
  return {
    freq_ranges: [],
    sample_rates: [],
    gains: [],
    antennas: [],
    bandwidths: [],
    duplex: "rx_only",
    ...overrides,
  };
}

export function deviceSet(overrides: Partial<DeviceSet> = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "virtual", key: "kraken5", label: "KrakenSDR" },
    capabilities: capabilities(),
    settings: {},
    status: "running",
    channels: [],
    overruns: 0,
    ...overrides,
  };
}

export function placed(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

export function laneStatus(
  lane: number,
  overrides: Partial<ArrayLaneStatus> = {},
): ArrayLaneStatus {
  return {
    lane,
    stream: lane,
    sync: "locked",
    delay_samples: 0,
    phase_deg: 0,
    gain_db: 0,
    coherence: 1,
    level_dbfs: -30,
    clipping: false,
    gaps: 0,
    gap_samples: 0,
    uncertain: 0,
    ...overrides,
  };
}

export function arrayStatus(array: string, overrides: Partial<ArrayStatus> = {}): ArrayStatus {
  return {
    node: array,
    lanes: [],
    tier: "time_sync",
    declared: "time_sync",
    tier_capped: false,
    sync: "locked",
    cal: "solved",
    phase_ready: true,
    center_hz: 145_000_000,
    sample_rate: 2_048_000,
    tuning: "together",
    gain: { kind: "manual", db: 30 },
    generation: 1,
    realigns: 0,
    dropped_samples: 0,
    events_lost: 0,
    ...overrides,
  };
}

export function detection(rangeKm: number): RadarDetection {
  return { cells: 1, doppler_hz: 40, range_km: rangeKm, range_rate_mps: -20, snr_db: 14 };
}

export function radarUpdate(detections: RadarDetection[], seq = 1): RadarUpdate {
  return {
    at: "2026-09-28T12:00:00Z",
    seq,
    axes: {
      batches: 64,
      carrier_hz: 98_000_000,
      cpi_ms: 500,
      doppler_rows: 128,
      doppler_step_hz: 2,
      gates: 256,
      hop_ms: 250,
      lanes: 5,
      range_step_m: 150,
      sample_rate_hz: 2_048_000,
    },
    health: {
      aoa: "ready",
      compute_ms: 40,
      discarded_cpis: 0,
      dropped_cpis: 0,
      dropped_reports: 0,
      dropped_samples: 0,
      dropped_tracks: 0,
      front_load: 0.2,
      gpu: false,
      gpu_failures: 0,
      lagged_updates: 0,
      load: 0.3,
      noise_floor_db: -110,
      reference: { fallback_frames: 0, locked: true, mode: "raw", quality_db: 30 },
      suppression_db: [],
      threads: 4,
      truncated_detections: 0,
      unsuppressed_groups: 0,
    },
    detections,
    tracks: [],
    truth: [],
  };
}

export function bearingRecord(
  finder: string,
  bearing: Partial<DfBearing>,
  at = "2026-09-28T12:00:00Z",
): DecodedRecord {
  return {
    at,
    channel: 4_294_967_295,
    device_set: 1,
    freq_hz: 145_000_000,
    origin: { node: finder, transmission: 0 },
    event: {
      kind: "df",
      data: { bearing_deg: 137, confidence: 0.8, sigma_deg: 3, lat: 52, lon: 13, ...bearing },
    },
  };
}
