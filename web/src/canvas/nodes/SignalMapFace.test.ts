import { describe, expect, it } from "vitest";
import type { WorkspaceSnapshot } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { placed } from "../../test/fixtures";
import { editSurveyBand } from "./SignalMapFace";

describe("editSurveyBand", () => {
  it("stores the band and applies, since the server reads it only on apply", () => {
    let snapshot = {
      version: 4,
      graph: { nodes: [placed("map", catalogBody("signal_map"))], edges: [] },
    } as unknown as WorkspaceSnapshot;
    const calls: string[] = [];
    editSurveyBand(
      {
        edit: (change) => {
          calls.push("edit");
          snapshot = change(snapshot);
        },
        apply: () => calls.push("apply"),
      },
      "map",
      25_000,
      6_250,
    );
    expect(calls).toEqual(["edit", "apply"]);
    expect(snapshot.graph.nodes[0]).toMatchObject({
      kind: "signal_map",
      data: { offset_hz: 25_000, bandwidth_hz: 6_250 },
    });
  });
});
