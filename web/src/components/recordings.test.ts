import { describe, expect, it } from "vitest";
import type { DeviceSet, RecordingInfo, RecordingStatus } from "../lib/types";
import {
  deriveRecordControl,
  formatBytes,
  formatDuration,
  formatTags,
  MAX_RECORDING_TAG_LEN,
  MAX_RECORDING_TAGS,
  matchesRecordingSearch,
  parseTags,
  recordingElapsedS,
} from "./recordings";

function set(over: Partial<DeviceSet>): DeviceSet {
  return {
    id: 1,
    status: "running",
    device: { driver: "virtual", key: "siggen", label: "Virtual SigGen" },
    settings: {},
    capabilities: {
      antennas: [],
      bandwidths: [],
      freq_ranges: [],
      gains: [],
      sample_rates: [],
    },
    channels: [],
    ...over,
  };
}

const status: RecordingStatus = {
  file: "siggen-20260809-120000",
  started_at: "2026-08-09T12:00:00Z",
  samples: 2_400_000,
  bytes: 19_200_000,
  overruns: 0,
};

describe("deriveRecordControl", () => {
  it("offers start only while the set is running", () => {
    expect(deriveRecordControl(set({}))).toEqual({ kind: "idle", canStart: true });
    expect(deriveRecordControl(set({ status: "idle" }))).toEqual({ kind: "idle", canStart: false });
    expect(deriveRecordControl(set({ status: "error" }))).toEqual({
      kind: "idle",
      canStart: false,
    });
  });

  it("reports recording with its live status", () => {
    expect(deriveRecordControl(set({ recording: status }))).toEqual({
      kind: "recording",
      status,
    });
  });

  it("keeps a faulted recording visible even when the set itself faulted", () => {
    const faulted = { ...status, error: "recording queue overflow" };
    expect(deriveRecordControl(set({ status: "error", recording: faulted }))).toEqual({
      kind: "recording",
      status: faulted,
    });
  });
});

describe("recordingElapsedS", () => {
  const startMs = Date.parse("2026-08-09T12:00:00Z");
  const rate = 2_400_000;

  it("measures wall-clock seconds since start", () => {
    expect(recordingElapsedS(status, startMs + 90_500, rate)).toBeCloseTo(90.5);
  });

  it("clamps clock skew and unparsable timestamps to zero", () => {
    expect(recordingElapsedS(status, startMs - 5_000, rate)).toBe(0);
    expect(recordingElapsedS({ ...status, started_at: "not a date" }, startMs, rate)).toBe(0);
  });

  it("freezes at the captured duration once the recording faulted", () => {
    const faulted = { ...status, error: "recording queue overflow" };
    expect(recordingElapsedS(faulted, startMs + 120_000, rate)).toBe(1);
    expect(recordingElapsedS(faulted, startMs + 240_000, rate)).toBe(1);
    expect(recordingElapsedS(faulted, startMs + 120_000, 0)).toBe(0);
  });
});

describe("formatDuration", () => {
  it("shows tenths below a minute", () => {
    expect(formatDuration(0)).toBe("0.0 s");
    expect(formatDuration(3.24)).toBe("3.2 s");
    expect(formatDuration(59.99)).toBe("1:00");
  });

  it("switches to m:ss and h:mm:ss", () => {
    expect(formatDuration(60)).toBe("1:00");
    expect(formatDuration(3_599)).toBe("59:59");
    expect(formatDuration(3_600)).toBe("1:00:00");
    expect(formatDuration(7_325)).toBe("2:02:05");
  });
});

describe("formatBytes", () => {
  it("scales through B / kB / MB / GB", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1_000)).toBe("1.0 kB");
    expect(formatBytes(19_200_000)).toBe("19.2 MB");
    expect(formatBytes(2_500_000_000)).toBe("2.50 GB");
  });
});

describe("parseTags", () => {
  it("trims, drops blanks and keeps the first spelling of a repeat", () => {
    expect(parseTags(" airband , AIRBAND ,, tower")).toEqual(["airband", "tower"]);
    expect(parseTags("   ")).toEqual([]);
    expect(formatTags(parseTags("airband,tower"))).toBe("airband, tower");
  });

  it("holds a tag and a list to what the server accepts", () => {
    expect(parseTags("t".repeat(MAX_RECORDING_TAG_LEN + 5))).toEqual([
      "t".repeat(MAX_RECORDING_TAG_LEN),
    ]);
    const many = Array.from({ length: MAX_RECORDING_TAGS + 4 }, (_, i) => `t${i}`).join(",");
    expect(parseTags(many)).toHaveLength(MAX_RECORDING_TAGS);
  });
});

describe("matchesRecordingSearch", () => {
  const recording = {
    id: 1,
    file: "siggen-20260809-120000",
    device_id: "virtual:file:/recordings/siggen",
    device_label: "Signal Generator",
    center_hz: 100e6,
    sample_rate: 2.048e6,
    samples: 4,
    bytes: 32,
    duration_s: 1,
    created_at: "2026-08-09T12:00:00Z",
    tags: ["airband", "tower"],
    note: "EDDF ground",
  } satisfies RecordingInfo;

  it("matches a file name, a tag or a note, case-insensitively", () => {
    expect(matchesRecordingSearch(recording, "")).toBe(true);
    expect(matchesRecordingSearch(recording, "  ")).toBe(true);
    expect(matchesRecordingSearch(recording, "SIGGEN")).toBe(true);
    expect(matchesRecordingSearch(recording, "tow")).toBe(true);
    expect(matchesRecordingSearch(recording, "eddf")).toBe(true);
    expect(matchesRecordingSearch(recording, "meteor")).toBe(false);
  });

  it("survives a recording that carries no annotation", () => {
    const bare = { ...recording, tags: [], note: null };
    expect(matchesRecordingSearch(bare, "airband")).toBe(false);
    expect(matchesRecordingSearch(bare, "siggen")).toBe(true);
  });
});
