import type { ComponentType } from "react";
import type { ToolCategory, ToolDescriptor } from "../lib/types";
import { AntennaPanel } from "./antenna/AntennaPanel";

/**
 * A tool's panel, registered against the id the server advertises it under.
 *
 * A tool that runs entirely in the browser carries its own `descriptor`; a tool backed by
 * `POST /api/tools/run` takes the descriptor the server sent, so the launcher never claims a
 * tool this build does not actually have.
 */
export interface ToolPanel {
  id: string;
  panel: ComponentType;
  descriptor?: ToolDescriptor;
}

export const TOOL_PANELS: readonly ToolPanel[] = [{ id: "antenna", panel: AntennaPanel }];

/** A tool the launcher can list. `panel` is null for a server tool this client has no UI for —
 * shown and explained rather than hidden, so a missing panel is a visible gap. */
export interface LaunchableTool {
  descriptor: ToolDescriptor;
  panel: ComponentType | null;
}

export interface ToolGroup {
  category: ToolCategory;
  label: string;
  tools: LaunchableTool[];
}

const CATEGORY_LABELS: Record<ToolCategory, string> = {
  instrument: "Instruments",
  calculator: "Calculators",
  reference: "Reference",
};

const CATEGORY_ORDER: readonly ToolCategory[] = ["instrument", "calculator", "reference"];

/** Every tool this session can open: the server's, plus the ones that need no server. */
export function launchableTools(descriptors: readonly ToolDescriptor[]): LaunchableTool[] {
  const served = descriptors.map((descriptor) => ({
    descriptor,
    panel: TOOL_PANELS.find((entry) => entry.id === descriptor.id)?.panel ?? null,
  }));
  const local = TOOL_PANELS.filter(
    (entry) =>
      entry.descriptor !== undefined &&
      !descriptors.some((descriptor) => descriptor.id === entry.id),
  ).map((entry) => ({
    descriptor: entry.descriptor as ToolDescriptor,
    panel: entry.panel,
  }));
  return [...served, ...local];
}

export function groupTools(tools: readonly LaunchableTool[]): ToolGroup[] {
  return CATEGORY_ORDER.map((category) => ({
    category,
    label: CATEGORY_LABELS[category],
    tools: tools
      .filter((tool) => tool.descriptor.category === category)
      .toSorted((left, right) => left.descriptor.name.localeCompare(right.descriptor.name)),
  })).filter((group) => group.tools.length > 0);
}

/** The tool a selection names, or the first one there is — a launcher with tools in it always
 * has something open. */
export function selectTool(
  groups: readonly ToolGroup[],
  selected: string | null,
): LaunchableTool | null {
  const tools = groups.flatMap((group) => group.tools);
  return tools.find((tool) => tool.descriptor.id === selected) ?? tools[0] ?? null;
}
