import { describe, expect, it } from "vitest";
import type { NodeKind } from "../lib/types";
import { carriesSettings, newNodeBody } from "./newNode";

const EVERY_KIND: Record<NodeKind, true> = {
  device: true,
  array: true,
  gps: true,
  channel: true,
  scope: true,
  speaker: true,
  map: true,
  signal_map: true,
  propagation: true,
  readout: true,
  decoder_log: true,
  dmr_trunk: true,
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
  df: true,
  passive_radar: true,
  hunt: true,
  triangulation: true,
  combiner: true,
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
      data: { kinds: [], stations: [], talkgroups: [], radios: [], min_duration_ms: 0 },
    });
  });

  it("starts a trunk system recording, matching the server default", () => {
    expect(newNodeBody("dmr_trunk")).toEqual({
      kind: "dmr_trunk",
      data: { protocol: "auto", record_calls: true },
    });
  });

  it("starts a channel not recording", () => {
    expect(newNodeBody("channel", { channelType: "dmr" })).toEqual({
      kind: "channel",
      data: { channel_type: "dmr", record_calls: false },
    });
  });
});
