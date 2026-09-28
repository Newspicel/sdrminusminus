import { GATE_TEXT, processorStatusOf } from "../../lib/arrays";
import { isStale, type ProcessorState } from "../../lib/processors";
import { clearAction, useRefusalStore } from "../../lib/refusals";
import type {
  ArrayStatus,
  PatchGraph,
  PatchNode,
  PatchNodeOf,
  ProcessorGate,
} from "../../lib/types";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
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
  if (arrayOf(graph, node) === null) {
    return NO_ARRAY;
  }
  const gate = processorGate(status, node);
  if (gate !== null) {
    return GATE_TEXT[gate].toLowerCase();
  }
  if (state !== undefined && isStale(state.receivedAt, now, periodMs)) {
    return STALE;
  }
  const lanes = status?.lanes.length ?? 0;
  return `${lanes} ${lanes === 1 ? "lane" : "lanes"}`;
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
