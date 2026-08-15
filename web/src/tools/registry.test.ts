import { describe, expect, it } from "vitest";
import type { ToolCategory, ToolDescriptor } from "../lib/types";
import { groupTools, launchableTools, selectTool, TOOL_PANELS } from "./registry";

function descriptor(
  id: string,
  name: string,
  category: ToolCategory = "calculator",
): ToolDescriptor {
  return { id, name, summary: `${name} summary`, category, needs_hardware: false };
}

describe("launchableTools", () => {
  it("gives every advertised tool its panel", () => {
    const tools = launchableTools([descriptor("antenna", "Antenna calculator")]);
    expect(tools).toHaveLength(1);
    expect(tools[0]?.panel).toBe(TOOL_PANELS.find((entry) => entry.id === "antenna")?.panel);
  });

  /** A tool this client cannot draw is still a tool the server has: hiding it would make the
   * gap invisible. */
  it("keeps a served tool that has no panel here", () => {
    const tools = launchableTools([descriptor("nanovna", "NanoVNA", "instrument")]);
    expect(tools).toHaveLength(1);
    expect(tools[0]?.panel).toBeNull();
  });

  it("lists nothing when the server offers nothing", () => {
    expect(launchableTools([])).toEqual([]);
  });
});

describe("groupTools", () => {
  it("groups by category in a fixed order and sorts by name inside a group", () => {
    const groups = groupTools(
      launchableTools([
        descriptor("z-calc", "Zed calculator"),
        descriptor("a-calc", "Alpha calculator"),
        descriptor("vna", "NanoVNA", "instrument"),
      ]),
    );
    expect(groups.map((group) => group.category)).toEqual(["instrument", "calculator"]);
    expect(groups[1]?.tools.map((tool) => tool.descriptor.name)).toEqual([
      "Alpha calculator",
      "Zed calculator",
    ]);
  });

  it("drops the categories nothing is in", () => {
    const groups = groupTools(launchableTools([descriptor("antenna", "Antenna calculator")]));
    expect(groups).toHaveLength(1);
    expect(groups[0]?.label).toBe("Calculators");
  });
});

describe("selectTool", () => {
  const groups = groupTools(
    launchableTools([descriptor("antenna", "Antenna"), descriptor("other", "Other")]),
  );

  it("opens the named tool", () => {
    expect(selectTool(groups, "other")?.descriptor.id).toBe("other");
  });

  it("falls back to the first tool rather than an empty pane", () => {
    expect(selectTool(groups, null)?.descriptor.id).toBe("antenna");
    expect(selectTool(groups, "gone")?.descriptor.id).toBe("antenna");
  });

  it("has nothing to open when there are no tools", () => {
    expect(selectTool([], "antenna")).toBeNull();
  });
});
