import { describe, expect, it } from "vitest";
import catalog from "../generated/patch-catalog.json";
import type { NodeKind } from "../lib/types";
import { carriesSettings, newNodeBody } from "./newNode";

const EVERY_KIND: Record<NodeKind, true> = {
  device: true,
  recording: true,
  signal_gen: true,
  gps: true,
  channel: true,
  scope: true,
  baseband_scope: true,
  speaker: true,
  map: true,
  signal_map: true,
  propagation: true,
  readout: true,
  decoder_log: true,
  dmr_trunk: true,
  audio_fx: true,
  spectrum_monitor: true,
  event_filter: true,
  event_output: true,
  video: true,
  recorder: true,
  audio_recorder: true,
  baseband_recorder: true,
  time_machine: true,
  network_export: true,
  export: true,
  scanner: true,
  hunt: true,
  satellite: true,
  triangulation: true,
  array: true,
  df: true,
  beamformer: true,
  passive_radar: true,
  stitch: true,
  spatial_spectrum: true,
  correlator: true,
  polarimeter: true,
};

const KINDS = Object.keys(EVERY_KIND) as NodeKind[];

describe("newNodeBody", () => {
  it.each(KINDS)("gives %s a body the server can parse", (kind) => {
    const body = newNodeBody(kind);
    expect(body.kind).toBe(kind);
    if (carriesSettings(kind)) {
      expect(body).toHaveProperty("data");
      expect((body as { data: unknown }).data).not.toBeUndefined();
    }
  });

  it("starts an event filter open, passing everything", () => {
    const body = newNodeBody("event_filter");
    expect(body).toEqual({
      kind: "event_filter",
      data: {
        mode: "keep",
        kinds: [],
        stations: [],
        talkgroups: [],
        radios: [],
        min_duration_ms: 0,
      },
    });
  });

  it("starts a trunk system recording, matching the server default", () => {
    expect(newNodeBody("dmr_trunk")).toEqual({
      kind: "dmr_trunk",
      data: { protocol: "auto", record_calls: true },
    });
  });

  it.each(["recorder", "audio_recorder", "baseband_recorder"] as const)(
    "starts a %s switched off",
    (kind) => {
      expect(newNodeBody(kind)).toEqual({ kind, data: { recording: false } });
    },
  );

  it("starts a channel not recording", () => {
    expect(newNodeBody("channel", { channelType: "dmr" })).toEqual({
      kind: "channel",
      data: { channel_type: "dmr", record_calls: false },
    });
  });

  it.each([
    "array",
    "df",
    "beamformer",
    "passive_radar",
    "stitch",
    "spatial_spectrum",
    "correlator",
    "polarimeter",
  ] as const)("starts a %s from a fresh copy of the catalog default", (kind) => {
    const listed = catalog.nodes.find((entry) => entry.kind === kind)?.default_body;
    const body = newNodeBody(kind);
    expect(listed).toBeDefined();
    expect(body).toEqual(listed);
    expect(body).not.toBe(listed);
    expect(newNodeBody(kind)).not.toBe(body);
  });

  it("gives a triangulation empty data", () => {
    expect(newNodeBody("triangulation")).toEqual({ kind: "triangulation", data: {} });
  });

  it("starts a monitor with a 70% minimum confidence", () => {
    expect(newNodeBody("spectrum_monitor")).toEqual({
      kind: "spectrum_monitor",
      data: { record_audio: true, min_confidence: 0.7 },
    });
  });
});
