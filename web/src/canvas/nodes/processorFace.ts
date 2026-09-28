import { GATE_TEXT, processorStatusOf } from "../../lib/arrays";
import { isStale, type ProcessorState } from "../../lib/processors";
import { clearAction, useRefusalStore } from "../../lib/refusals";
import type {
  ArrayStatus,
  PatchGraph,
  PatchNode,
  PatchNodeOf,
  ProcessorGate,
  ProcessorStatus,
} from "../../lib/types";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { arrayWiredLanes, patchNode } from "../graph";
import { type SettingsKind, type SettingsOf, settingsOf } from "../newNode";

export const NO_ARRAY = "no array";
export const STALE = "stale";
export const NO_CATALOG = "Node types not loaded";
export const SETTINGS_ACTION = "Settings";

export function processorGate(status: ArrayStatus | undefined, node: string): ProcessorGate | null {
  return processorStatusOf(status, node)?.gated ?? null;
}

export function processorSubtitle(
  graph: PatchGraph,
  node: string,
  status: ArrayStatus | undefined,
  state: ProcessorState | undefined,
  now: number,
  periodMs: number,
): string {
  const array = arrayOf(graph, node);
  if (array === null) {
    return NO_ARRAY;
  }
  const gate = processorGate(status, node);
  if (gate !== null) {
    return GATE_TEXT[gate].toLowerCase();
  }
  if (state !== undefined && isStale(state.receivedAt, now, periodMs)) {
    return STALE;
  }
  const lanes = processorLanes(graph, array, status);
  return `${lanes} ${lanes === 1 ? "lane" : "lanes"}`;
}

export function processorLanes(
  graph: PatchGraph,
  array: string | null,
  status: ArrayStatus | undefined,
): number {
  if (array === null) {
    return 0;
  }
  return status?.lanes.length ?? arrayWiredLanes(graph, array);
}

export interface FaultRow {
  label: string;
  count: number;
  title: string;
}

export function processorFaults(status: ProcessorStatus | null): FaultRow[] {
  if (status === null) {
    return [];
  }
  return [
    { label: "Drops", count: status.dropped_samples, title: "Samples dropped" },
    { label: "Lost", count: status.dropped_reports, title: "Reports lost" },
    {
      label: "Cut",
      count: (status.truncated ?? 0) + status.lane_overflows,
      title: "Results cut at their limit",
    },
    { label: "Fails", count: status.solver_failures, title: "Solver failures" },
    { label: "Mismatch", count: status.lane_mismatch, title: "Blocks with the wrong lane count" },
  ].filter((row) => row.count > 0);
}

export function ageLabel(receivedAt: number | undefined, now: number): string {
  if (receivedAt === undefined) {
    return "-";
  }
  const seconds = Math.max(0, now - receivedAt) / 1_000;
  if (seconds < 10) {
    return `${seconds.toFixed(1)} s`;
  }
  if (seconds < 60) {
    return `${Math.floor(seconds)} s`;
  }
  return `${Math.floor(seconds / 60)} min`;
}

type ProcessorNode = PatchNodeOf<SettingsKind>;

export function withSettings<K extends SettingsKind>(
  stored: PatchNodeOf<K>,
  base: SettingsOf<K>,
  next: Partial<SettingsOf<K>>,
): PatchNode {
  const typed = stored as unknown as ProcessorNode;
  return { ...typed, data: { ...typed.data, settings: { ...base, ...next } } } as PatchNode;
}

export function useProcessorEdit<K extends SettingsKind>(
  node: PatchNodeOf<K>,
): (next: Partial<SettingsOf<K>>) => void {
  const workspace = useWorkspaceContext();
  const typed = node as unknown as ProcessorNode;
  return (next) => {
    const catalog = workspace.context.catalog;
    if (settingsOf(node, catalog) === null) {
      useRefusalStore.getState().flag(typed.id, NO_CATALOG, "action", SETTINGS_ACTION);
      return;
    }
    clearAction(typed.id, SETTINGS_ACTION);
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, typed.id, (stored) => {
        if (stored.kind !== typed.kind) {
          return stored;
        }
        const current = stored as unknown as PatchNodeOf<K>;
        const base = settingsOf(current, catalog);
        return base === null ? stored : withSettings(current, base, next);
      }),
    }));
    workspace.apply();
  };
}
