import { afterEach, describe, expect, it } from "vitest";
import { useArrayStore } from "../../lib/arrays";
import { useProcessorStore } from "../../lib/processors";
import type { PatchGraph, RadarTrack } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { arrayStatus, detection, laneStatus, placed, radarUpdate } from "../../test/fixtures";
import { RadarFace } from "./RadarFace";

const RADAR = placed("radar", catalogBody("passive_radar"));

function graph(tx: boolean): PatchGraph {
  return {
    nodes: [placed("arr", catalogBody("array")), placed("site", catalogBody("gps")), RADAR],
    edges: [
      { from: { node: "arr", port: "array" }, to: { node: "radar", port: "array" } },
      ...(tx
        ? [{ from: { node: "site", port: "position" }, to: { node: "radar", port: "tx" } }]
        : []),
    ],
  };
}

const TRACK: RadarTrack = {
  id: 3,
  state: "confirmed",
  range_km: 42.1,
  range_rate_mps: -120,
  doppler_hz: 40,
  accel_mps2: 0,
  range_sigma_m: 20,
  rate_sigma_mps: 2,
  snr_db: 14,
  looks: 6,
  misses: 0,
  trail: [],
  aoa: { azimuth_deg: 231, sigma_deg: 3, quality: 0.8 },
  adsb: { icao: "3C6444", callsign: "DLH4AB" },
};

afterEach(() => {
  useProcessorStore.getState().reset();
  useArrayStore.getState().reset();
});

describe("RadarFace", () => {
  it("labels both axes and the colour scale", () => {
    const html = renderFace(RadarFace, RADAR, { graph: graph(true) });
    for (const unit of [">km<", ">Hz<", ">m/s<", ">dB<"]) {
      expect(html).toContain(unit);
    }
  });

  it("says no tx when the transmitter is unwired", () => {
    expect(renderFace(RadarFace, RADAR, { graph: graph(false) })).toContain("no tx");
    expect(renderFace(RadarFace, RADAR, { graph: graph(true) })).not.toContain("no tx");
  });

  it("lists tracks with their bearing frame and ADS-B match", () => {
    useArrayStore
      .getState()
      .seed([arrayStatus("arr", { lanes: [0, 1, 2, 3, 4].map((lane) => laneStatus(lane)) })]);
    useProcessorStore.setState({
      byNode: {
        radar: {
          reading: {
            type: "passive_radar",
            reading: { ...radarUpdate([detection(12)]), tracks: [TRACK] },
          },
          receivedAt: Date.now(),
        },
      },
    });
    const html = renderFace(RadarFace, RADAR, { graph: graph(true) });
    expect(html).toContain("T3");
    expect(html).toContain("DLH4AB");
    expect(html).toContain('title="Array frame"');
    expect(html).toContain("FM 98.00 MHz");
    expect(html).not.toContain("Outside cal table");
  });

  it("shows when AoA steers outside the array table", () => {
    useProcessorStore.setState({
      byNode: {
        radar: {
          reading: {
            type: "passive_radar",
            reading: { ...radarUpdate([]), problems: [{ kind: "table_out_of_range" }] },
          },
          receivedAt: Date.now(),
        },
      },
    });
    const html = renderFace(RadarFace, RADAR, { graph: graph(true) });
    expect(html.split("Outside cal table")).toHaveLength(2);
    expect(html).toContain('title="Bearings use the ideal geometry"');
    expect(html).toContain("FM 98.00 MHz");
  });
});
