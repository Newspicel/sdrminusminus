import type { ComponentType } from "react";
import type { ToolCategory, ToolDescriptor } from "../lib/types";
import { AntennaPanel } from "./antenna/AntennaPanel";
import { CpsPanel } from "./cps/CpsPanel";
import { NanoVnaPanel } from "./nanovna/NanoVnaPanel";

export interface ToolPanel {
  id: string;
  panel: ComponentType;
  descriptor?: ToolDescriptor;
  size?: ToolSize;
}

export type ToolSize = "standard" | "full";

export const TOOL_PANELS: readonly ToolPanel[] = [
  { id: "antenna", panel: AntennaPanel },
  {
    id: "cps",
    panel: CpsPanel,
    size: "full",
    descriptor: {
      id: "cps",
      name: "Radio programmer",
      summary: "Read, edit and write radio codeplugs, and copy them between radios",
      category: "instrument",
      needs_hardware: true,
    },
  },
  { id: "nanovna", panel: NanoVnaPanel, size: "full" },
];

export function toolSize(id: string | null): ToolSize {
  return TOOL_PANELS.find((entry) => entry.id === id)?.size ?? "standard";
}

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

export function findTool(
  tools: readonly LaunchableTool[],
  id: string | null,
): LaunchableTool | null {
  return tools.find((tool) => tool.descriptor.id === id) ?? null;
}
