import { describe, expect, it } from "vitest";
import type { WorkspaceSnapshot } from "../lib/types";
import { WorkspaceDrafts } from "./workspaceDrafts";

function snapshot(label: string): WorkspaceSnapshot {
  return {
    version: 2,
    graph: { nodes: [], edges: [] },
    rack: {},
    settings: { band_region: label },
  };
}

describe("workspace drafts", () => {
  it("finishes each workspace independently before a later save", () => {
    const drafts = new WorkspaceDrafts();
    const firstA = drafts.stage(1, snapshot("a1"), 1);
    const firstB = drafts.stage(2, snapshot("b1"), 4);

    drafts.accepted(1, 2);
    expect(drafts.finish(1, firstA.generation)).toBe(true);
    expect(drafts.get(2)).toEqual(firstB);

    // A refetched after its queue drained. Its next save must start from that current revision,
    // not from revision 2 retained merely because B was also pending.
    const secondA = drafts.stage(1, snapshot("a2"), 9);
    expect(secondA.revision).toBe(9);
    expect(drafts.finish(2, firstB.generation)).toBe(true);
    expect(drafts.get(1)).toEqual(secondA);
  });

  it("keeps a newer generation when an earlier write finishes", () => {
    const drafts = new WorkspaceDrafts();
    const first = drafts.stage(1, snapshot("a1"), 1);
    drafts.accepted(1, 2);
    const second = drafts.stage(1, snapshot("a2"), 2);

    expect(drafts.finish(1, first.generation)).toBe(false);
    expect(drafts.get(1)).toEqual(second);
    expect(drafts.finish(1, second.generation)).toBe(true);
  });
});
