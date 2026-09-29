import { describe, expect, it } from "vitest";
import type { NodeKind, PatchCatalog, PatchNodeOf } from "../lib/types";
import { CATALOG } from "../test/catalog";
import {
  carriesSettings,
  defaultBody,
  huntSweepOf,
  newNodeBody,
  settingsOf,
  startsOnItsOwn,
} from "./newNode";

const KINDS = CATALOG.nodes.map((entry) => entry.kind as NodeKind);

describe("newNodeBody", () => {
  it("takes every body from the catalog", () => {
    for (const entry of CATALOG.nodes) {
      if (entry.kind === "channel") {
        continue;
      }
      expect(newNodeBody(CATALOG, entry.kind as NodeKind)).toEqual(entry.default_body);
    }
  });

  it("hands out a fresh copy each time", () => {
    const listed = CATALOG.nodes.find((entry) => entry.kind === "df")?.default_body;
    const first = newNodeBody(CATALOG, "df");
    expect(first).toEqual(listed);
    expect(first).not.toBe(listed);
    expect(newNodeBody(CATALOG, "df")).not.toBe(first);
  });

  it("returns null for an unknown kind", () => {
    const empty: PatchCatalog = { nodes: [] };
    expect(newNodeBody(empty, "array")).toBeNull();
    expect(defaultBody(empty, "scope")).toBeNull();
  });

  it("starts a channel of the picked type, not recording", () => {
    expect(newNodeBody(CATALOG, "channel", { channelType: "dmr" })).toEqual({
      kind: "channel",
      data: { channel_type: "dmr", record_calls: false },
    });
    expect(newNodeBody(CATALOG, "channel")).toEqual({
      kind: "channel",
      data: { channel_type: "nfm", record_calls: false },
    });
  });

  it.each(KINDS)("gives %s a body of its own kind", (kind) => {
    const body = newNodeBody(CATALOG, kind);
    expect(body?.kind).toBe(kind);
    if (carriesSettings(CATALOG, kind)) {
      expect((body as { data?: unknown }).data).toBeDefined();
    }
  });

  it("knows which kinds carry no settings", () => {
    expect(carriesSettings(CATALOG, "scope")).toBe(false);
    expect(carriesSettings(CATALOG, "triangulation")).toBe(true);
    expect(carriesSettings({ nodes: [] }, "df")).toBe(false);
  });

  it("starts only a signal generator on its own", () => {
    expect(startsOnItsOwn("signal_gen")).toBe(true);
    expect(startsOnItsOwn("device")).toBe(false);
  });
});

describe("settingsOf", () => {
  it("reads stored settings and falls back to the catalog default", () => {
    const stored = newNodeBody(CATALOG, "df") as PatchNodeOf<"df">;
    const node = {
      ...stored,
      id: "df:1",
      position: { x: 0, y: 0 },
      data: { settings: { ...stored.data.settings, report_ms: 900 } },
    } as PatchNodeOf<"df">;
    expect(settingsOf(node, CATALOG)?.report_ms).toBe(900);
    const bare = { ...node, data: {} } as PatchNodeOf<"df">;
    expect(settingsOf(bare, CATALOG)).toEqual(stored.data.settings);
  });
});

describe("huntSweepOf", () => {
  it("reads the hunt's own sweep and falls back to the catalog default", () => {
    const walk = {
      id: "hunt:1",
      position: { x: 0, y: 0 },
      kind: "hunt",
      data: {},
    } as PatchNodeOf<"hunt">;
    const fallback = huntSweepOf(walk, CATALOG);
    expect(fallback?.beamwidth_deg).toBe(60);
    const mounted = { ...walk, data: { sweep: { ...fallback, mount_offset_deg: -90 } } };
    expect(huntSweepOf(mounted as PatchNodeOf<"hunt">, CATALOG)?.mount_offset_deg).toBe(-90);
    expect(huntSweepOf(walk, { nodes: [] })).toBeNull();
  });
});
