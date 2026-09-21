import { describe, expect, it } from "vitest";
import type { DecodedRecordOf } from "../../lib/types";
import { monitorTransmissions } from "./spectrumMonitor";

function record(
  node: string,
  id: number,
  state: "started" | "completed",
): DecodedRecordOf<"transmission"> {
  return {
    origin: { node, transmission: id },
    device_set: 1,
    channel: 0,
    at: "2026-09-21T00:00:00Z",
    freq_hz: 145000000,
    event: {
      kind: "transmission",
      data: {
        id,
        state,
        start_sample: 0,
        end_sample: 48000,
        sample_rate_hz: 48000,
        duration_ms: 1000,
        signal: {
          modulation: "fm",
          confidence: 0.8,
          frequency_hz: 145000000,
          center_offset_hz: 0,
          bandwidth_hz: 12500,
          snr_db: 20,
          features: {
            envelope_variation: 0,
            duty: 1,
            keying_depth_db: 0,
            spectral_asymmetry: 0,
            carrier_db: 0,
            spectral_flatness: 0,
            frequency_levels: 0,
            frequency_spread_hz: 0,
            square_line_db: 0,
            quartic_line_db: 0,
          },
        },
      },
    },
  };
}

describe("monitorTransmissions", () => {
  it("keeps the latest state of each transmission on this node", () => {
    const rows = monitorTransmissions(
      [
        record("a", 1, "completed"),
        record("b", 2, "started"),
        record("a", 3, "started"),
        record("a", 1, "started"),
      ],
      "a",
    );
    expect(rows.map((row) => [row.id, row.state])).toEqual([
      [1, "completed"],
      [3, "started"],
    ]);
  });
});
