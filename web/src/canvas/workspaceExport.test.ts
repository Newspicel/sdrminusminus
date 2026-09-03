import { describe, expect, it } from "vitest";
import type { WorkspaceExport } from "../lib/types";
import { parseWorkspaceExport } from "./workspaceExport";

const document = (): WorkspaceExport => ({
  version: 1,
  name: "Airband Watch",
  snapshot: {
    version: 3,
    graph: {
      nodes: [
        { id: "dev", kind: "device", data: {}, position: { x: 0, y: 0 } },
        { id: "scope", kind: "scope", position: { x: 400, y: 0 } },
      ],
      edges: [{ from: { node: "dev", port: "iq" }, to: { node: "scope", port: "iq" } }],
    },
  },
  state: { version: 1, devices: [{ node: "dev", settings: { center_hz: 145_500_000 } }] },
});

describe("reading a workspace file", () => {
  it("keeps the name, the layout and the tuning the file was written with", () => {
    const read = parseWorkspaceExport(JSON.stringify(document()));

    expect(read).toEqual(document());
  });

  it("lifts a layout written before the scanner grew its control wire", () => {
    const older = document();
    older.snapshot.graph.nodes.push({ id: "scan", kind: "scanner", position: { x: 0, y: 400 } });
    older.snapshot.graph.edges?.push({
      from: { node: "dev", port: "iq" },
      to: { node: "scan", port: "iq" },
    });

    const read = parseWorkspaceExport(JSON.stringify(older));

    expect(read.snapshot.graph.edges).toEqual([
      { from: { node: "dev", port: "iq" }, to: { node: "scope", port: "iq" } },
      { from: { node: "scan", port: "control" }, to: { node: "dev", port: "control" } },
    ]);
  });

  it("refuses a file that is not a workspace instead of posting it", () => {
    expect(() => parseWorkspaceExport("not json")).toThrow(/not JSON/);
    expect(() => parseWorkspaceExport("[]")).toThrow(/not a workspace/);
    expect(() => parseWorkspaceExport(JSON.stringify({ version: 1, name: "No layout" }))).toThrow(
      /not a workspace/,
    );
    const unnamed = { ...document(), name: "  " };
    expect(() => parseWorkspaceExport(JSON.stringify(unnamed))).toThrow(/not a workspace/);
  });
});
