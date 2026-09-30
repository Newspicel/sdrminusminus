import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { DeviceSet, PatchGraph, PatchNode, RecordingInfo } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace, stubWorkspace } from "../../test/faceHarness";
import { portsOf } from "../graph";
import { makeArray } from "../makeArray";
import { arrayLaneRows } from "./arrayNode";
import { RecordingFace, RecordingFacts } from "./RecordingFace";
import { recordingDeviceId } from "./recordingNode";

const LANES = 5;

const take: PatchNode = {
  id: "rec",
  kind: "recording",
  data: { recording: "take" },
  position: { x: 0, y: 0 },
};

function played(lanes: number): DeviceSet {
  return {
    id: 7,
    device: { driver: "recording", key: "take", label: "take" },
    capabilities: {
      freq_ranges: [],
      sample_rates: [2_400_000],
      gains: [],
      antennas: [],
      bandwidths: [],
      extra: [],
      duplex: "rx_only",
      rx_streams: lanes,
      coherence: "time_sync",
      noise_source: "replayed",
    },
    settings: {},
    status: "running",
    channels: [],
    overruns: 0,
  };
}

function collection(lanes: number): RecordingInfo {
  return {
    id: 1,
    file: "take",
    device_id: recordingDeviceId("take"),
    device_label: "virtual:kraken5",
    center_hz: 433_920_000,
    sample_rate: 2_400_000,
    samples: 2_400_000,
    bytes: 2_400_000 * 8 * lanes,
    duration_s: 1,
    created_at: "2026-09-29T12:00:00Z",
    lanes,
  };
}

const alone: PatchGraph = { nodes: [take], edges: [] };

describe("RecordingFace", () => {
  it("draws one iq output per recorded lane once the collection plays", () => {
    const devices = new Map([["rec", played(LANES)]]);
    const context = stubWorkspace({ graph: alone, devices }).context;
    expect(portsOf(context, alone, take).map((port) => port.name)).toEqual([
      "iq",
      "iq2",
      "iq3",
      "iq4",
      "iq5",
    ]);
    const single = stubWorkspace({ graph: alone, devices: new Map([["rec", played(1)]]) });
    expect(portsOf(single.context, alone, take).map((port) => port.name)).toEqual(["iq"]);
  });

  it("offers Make array for a playing collection and wires every lane", () => {
    const devices = new Map([["rec", played(LANES)]]);
    expect(renderFace(RecordingFace, take, { graph: alone, devices })).toContain("Make array");
    const single = renderFace(RecordingFace, take, {
      graph: alone,
      devices: new Map([["rec", played(1)]]),
    });
    expect(single).not.toContain("Make array");

    const body = catalogBody("array");
    if (body.kind !== "array") {
      throw new Error("an array body");
    }
    const made = makeArray(alone, "rec", LANES, body, "arr");
    const rows = arrayLaneRows(made, devices, "arr", undefined);
    expect(rows.map((row) => row.sourceLabel)).toEqual([
      "take iq1",
      "take iq2",
      "take iq3",
      "take iq4",
      "take iq5",
    ]);
    expect(renderFace(RecordingFace, take, { graph: made, devices })).not.toContain("Make array");
  });

  it("reads out the lane count of a collection only", () => {
    const five = renderToStaticMarkup(
      <RecordingFacts recording={collection(LANES)} separated={false} />,
    );
    expect(five).toContain("Lanes");
    expect(five).toContain(">5<");
    const one = renderToStaticMarkup(
      <RecordingFacts recording={collection(1)} separated={false} />,
    );
    expect(one).not.toContain("Lanes");
    expect(one).toContain("Centre");
  });
});
