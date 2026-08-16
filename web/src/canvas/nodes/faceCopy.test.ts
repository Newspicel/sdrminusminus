import { describe, expect, it } from "vitest";
import type { PatchGraph } from "../../lib/types";
import { CHANNEL_IDLE, faceEmptyText, RADIO_IDLE } from "./faceCopy";

const UNWIRED = "Wire a channel's audio out to this speaker.";

const graph: PatchGraph = {
  nodes: [],
  edges: [
    { from: { node: "nfm", port: "audio" }, to: { node: "spk", port: "audio" } },
    { from: { node: "dev", port: "iq" }, to: { node: "rec", port: "iq" } },
  ],
};

describe("faceEmptyText", () => {
  it("asks for a wire only when the port has none", () => {
    expect(faceEmptyText(graph, "spk", "events", UNWIRED)).toBe(UNWIRED);
    expect(faceEmptyText({ nodes: [], edges: [] }, "spk", "audio", UNWIRED)).toBe(UNWIRED);
  });

  it("blames the idle source when the wire is drawn but nothing arrives", () => {
    expect(faceEmptyText(graph, "spk", "audio", UNWIRED)).toBe(CHANNEL_IDLE);
    expect(faceEmptyText(graph, "rec", "iq", UNWIRED)).toBe(RADIO_IDLE);
  });
});
