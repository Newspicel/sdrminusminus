import { describe, expect, it } from "vitest";
import type { ToolCategory, ToolDescriptor } from "../lib/types";
import { findTool, groupTools, launchableTools, TOOL_PANELS, toolSize } from "./registry";

function descriptor(
  id: string,
  name: string,
  category: ToolCategory = "calculator",
): ToolDescriptor {
  return { id, name, summary: `${name} summary`, category, needs_hardware: false };
}

const CLIENT_ONLY = TOOL_PANELS.filter((entry) => entry.descriptor !== undefined).length;

function served(tools: ReturnType<typeof launchableTools>, id: string) {
  return tools.find((tool) => tool.descriptor.id === id);
}

describe("launchableTools", () => {
  it("gives every advertised tool its panel", () => {
    const tools = launchableTools([descriptor("antenna", "Antenna calculator")]);
    expect(tools).toHaveLength(1 + CLIENT_ONLY);
    expect(served(tools, "antenna")?.panel).toBe(
      TOOL_PANELS.find((entry) => entry.id === "antenna")?.panel,
    );
  });

  it("opens the NanoVNA instrument panel", () => {
    const tools = launchableTools([descriptor("nanovna", "NanoVNA", "instrument")]);
    expect(served(tools, "nanovna")?.panel).toBe(
      TOOL_PANELS.find((entry) => entry.id === "nanovna")?.panel,
    );
  });

  it("still offers a panel this client owns when the server advertises no tool", () => {
    const tools = launchableTools([]);
    expect(tools).toHaveLength(CLIENT_ONLY);
    expect(served(tools, "cps")?.panel).toBe(
      TOOL_PANELS.find((entry) => entry.id === "cps")?.panel,
    );
  });

  it("does not list a client panel twice when the server also advertises it", () => {
    const tools = launchableTools([descriptor("cps", "Radio programmer", "instrument")]);
    expect(tools.filter((tool) => tool.descriptor.id === "cps")).toHaveLength(1);
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
    expect(groups[0]?.tools.map((tool) => tool.descriptor.id)).toContain("cps");
  });

  it("drops the categories nothing is in", () => {
    const groups = groupTools([
      {
        descriptor: descriptor("antenna", "Antenna calculator"),
        panel: null,
      },
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.label).toBe("Calculators");
  });
});

describe("toolSize", () => {
  it("gives the instrument the whole window and the calculator a dialog", () => {
    expect(toolSize("nanovna")).toBe("full");
    expect(toolSize("cps")).toBe("full");
    expect(toolSize("antenna")).toBe("standard");
  });

  it("falls back to the dialog for a tool this client has no panel for", () => {
    expect(toolSize("unknown")).toBe("standard");
    expect(toolSize(null)).toBe("standard");
  });
});

describe("findTool", () => {
  const tools = launchableTools([descriptor("antenna", "Antenna"), descriptor("other", "Other")]);

  it("opens the named tool", () => {
    expect(findTool(tools, "other")?.descriptor.id).toBe("other");
  });

  it("finds nothing for a tool this build does not have", () => {
    expect(findTool(tools, "gone")).toBeNull();
    expect(findTool(tools, null)).toBeNull();
    expect(findTool([], "antenna")).toBeNull();
  });
});
