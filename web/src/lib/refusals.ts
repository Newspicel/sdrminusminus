import { create } from "zustand";
import { omitNodes } from "./byNode";
import { describeError } from "./diagnostics";
import { toastError } from "./toasts";
import type { PatchApplyReport } from "./types";

export type RefusalSource = "apply" | "wire" | "action";

export interface Refusal {
  reason: string;
  source: RefusalSource;
  at: number;
  action?: string;
}

export interface RefusalStore {
  byNode: Readonly<Record<string, readonly Refusal[]>>;
  fromReport: (report: PatchApplyReport) => void;
  flag: (node: string, reason: string, source: RefusalSource, action?: string) => void;
  dismiss: (node: string) => void;
  forget: (nodes: readonly string[]) => void;
  reset: () => void;
}

export const WIRE_REFUSAL_MS = 6_000;

export const MAX_REFUSALS = 8;

export const NOT_CONNECTED = "radio not connected";

type ByNode = Readonly<Record<string, readonly Refusal[]>>;

function kept(byNode: ByNode, keep: (refusal: Refusal, node: string) => boolean): ByNode {
  let changed = false;
  const next: Record<string, readonly Refusal[]> = {};
  for (const [node, list] of Object.entries(byNode)) {
    const rest = list.filter((refusal) => keep(refusal, node));
    changed ||= rest.length !== list.length;
    if (rest.length > 0) {
      next[node] = rest;
    }
  }
  return changed ? next : byNode;
}

export function wireExpired(refusal: Refusal, now: number): boolean {
  return refusal.source === "wire" && now - refusal.at > WIRE_REFUSAL_MS;
}

function appended(byNode: ByNode, node: string, refusal: Refusal): ByNode {
  const live = (byNode[node] ?? []).filter((held) => !wireExpired(held, refusal.at));
  return { ...byNode, [node]: [...live, refusal].slice(-MAX_REFUSALS) };
}

export const useRefusalStore = create<RefusalStore>((set) => ({
  byNode: {},
  fromReport: (report) =>
    set((state) => {
      const at = Date.now();
      let next = kept(state.byNode, (refusal) => refusal.source !== "apply");
      for (const refusal of report.refused ?? []) {
        next = appended(next, refusal.node, { reason: refusal.reason, source: "apply", at });
      }
      for (const node of report.absent ?? []) {
        next = appended(next, node, { reason: NOT_CONNECTED, source: "apply", at });
      }
      return { byNode: next };
    }),
  flag: (node, reason, source, action) =>
    set((state) => ({
      byNode: appended(state.byNode, node, {
        reason,
        source,
        at: Date.now(),
        ...(action === undefined ? {} : { action }),
      }),
    })),
  dismiss: (node) => set((state) => ({ byNode: omitNodes(state.byNode, [node]) })),
  forget: (nodes) => set((state) => ({ byNode: omitNodes(state.byNode, nodes) })),
  reset: () => set({ byNode: {} }),
}));

export function visibleRefusal(list: readonly Refusal[] | undefined, now: number): Refusal | null {
  let newest: Refusal | null = null;
  for (const refusal of list ?? []) {
    if (!wireExpired(refusal, now) && (newest === null || refusal.at >= newest.at)) {
      newest = refusal;
    }
  }
  return newest;
}

export function wireShownUntil(list: readonly Refusal[] | undefined): number | null {
  let until: number | null = null;
  for (const refusal of list ?? []) {
    if (refusal.source === "wire") {
      until = Math.max(until ?? 0, refusal.at + WIRE_REFUSAL_MS);
    }
  }
  return until;
}

export function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : describeError(error);
}

export function flagAction(node: string, action: string, error: unknown): void {
  useRefusalStore.getState().flag(node, `${action}: ${messageOf(error)}`, "action", action);
}

export function failAction(node: string, action: string, error: unknown): void {
  toastError(error);
  flagAction(node, action, error);
}

export function clearAction(node: string, action: string): void {
  useRefusalStore.setState((state) => ({
    byNode: kept(
      state.byNode,
      (refusal, held) => held !== node || refusal.source !== "action" || refusal.action !== action,
    ),
  }));
}
