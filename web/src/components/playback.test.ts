import { describe, expect, it } from "vitest";
import type { DeviceSet, PlaybackStatus } from "../lib/types";
import {
  formatClock,
  isLooping,
  playbackPositionAt,
  playbackProgress,
  samplesToSeconds,
} from "./playback";

function status(over: Partial<PlaybackStatus> = {}): PlaybackStatus {
  return { position_samples: 0, total_samples: 48_000, paused: false, ...over };
}

function set(over: Partial<DeviceSet> = {}): DeviceSet {
  return {
    id: 1,
    status: "running",
    device: { driver: "virtual", key: "file:rec_1", label: "rec_1" },
    settings: { sample_rate: 48_000 },
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

describe("playbackProgress", () => {
  it("reports the fraction played", () => {
    expect(playbackProgress(status({ position_samples: 12_000 }))).toBeCloseTo(0.25);
  });

  it("is 0 for a recording with no samples", () => {
    expect(playbackProgress(status({ total_samples: 0, position_samples: 0 }))).toBe(0);
  });

  it("never exceeds 1, even if the position outran the total", () => {
    expect(playbackProgress(status({ position_samples: 99_999 }))).toBe(1);
  });
});

describe("playbackPositionAt", () => {
  it("advances on the clock between snapshots", () => {
    const at = playbackPositionAt(status({ position_samples: 1_000 }), 500, 48_000, true);
    expect(at).toBe(1_000 + 24_000);
  });

  it("holds while paused, however long ago the snapshot was", () => {
    const at = playbackPositionAt(
      status({ position_samples: 1_000, paused: true }),
      60_000,
      48_000,
      true,
    );
    expect(at).toBe(1_000);
  });

  it("wraps with the loop instead of running past the end", () => {
    // 1.5 recordings' worth of wall clock from the start of a one-second file.
    const at = playbackPositionAt(status({ position_samples: 0 }), 1_500, 48_000, true);
    expect(at).toBe(24_000);
  });

  it("holds at the end when looping is off", () => {
    const at = playbackPositionAt(status({ position_samples: 0 }), 5_000, 48_000, false);
    expect(at).toBe(48_000);
  });

  it("never runs backwards or divides by a missing rate", () => {
    expect(playbackPositionAt(status({ position_samples: 100 }), -5_000, 48_000, true)).toBe(100);
    expect(playbackPositionAt(status({ position_samples: 100 }), 5_000, 0, true)).toBe(100);
    expect(playbackPositionAt(status({ total_samples: 0 }), 5_000, 48_000, true)).toBe(0);
  });

  /// A position past the end (a torn pair reindexed shorter) must clamp, not report a bar
  /// wider than its track.
  it("clamps a reported position beyond the recording", () => {
    expect(
      playbackPositionAt(status({ position_samples: 99_999, paused: true }), 0, 48_000, false),
    ).toBe(48_000);
  });
});

describe("samplesToSeconds", () => {
  it("converts at the set's rate", () => {
    expect(samplesToSeconds(96_000, 48_000)).toBe(2);
  });

  // A set whose rate has not arrived yet must not put NaN in the clock.
  it("is 0 without a rate", () => {
    expect(samplesToSeconds(96_000, 0)).toBe(0);
  });
});

describe("formatClock", () => {
  it("reads m:ss at a fixed width, so it does not jitter while running", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(9.9)).toBe("0:09");
    expect(formatClock(64)).toBe("1:04");
    expect(formatClock(599)).toBe("9:59");
  });

  it("grows an hours field only when there are hours", () => {
    expect(formatClock(3_600)).toBe("1:00:00");
    expect(formatClock(3_725)).toBe("1:02:05");
  });

  it("never reads negative", () => {
    expect(formatClock(-5)).toBe("0:00");
  });
});

describe("isLooping", () => {
  it("reads the loop extra", () => {
    expect(isLooping(set({ settings: { extra: [{ name: "loop", value: false }] } }))).toBe(false);
    expect(isLooping(set({ settings: { extra: [{ name: "loop", value: true }] } }))).toBe(true);
  });

  // Matches `ExtraSetting::Bool { default: true }` on the playback backend: a set whose extras
  // have not arrived must not draw the toggle off and then flip it.
  it("defaults to on when the setting is absent or not a boolean", () => {
    expect(isLooping(set())).toBe(true);
    expect(isLooping(set({ settings: { extra: [] } }))).toBe(true);
    expect(isLooping(set({ settings: { extra: [{ name: "loop", value: "yes" }] } }))).toBe(true);
  });
});
