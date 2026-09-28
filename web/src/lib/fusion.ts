import { useMutation, useQuery } from "@tanstack/react-query";
import { useCallback } from "react";
import { create } from "zustand";
import { fusionQuery, resetFusion } from "./api";
import { omitNodes } from "./byNode";
import { clearAction, failAction } from "./refusals";
import type { DfFusionState, ServerEvent } from "./types";
import { useSeed } from "./useSeed";

export interface FusionStore {
  byNode: Readonly<Record<string, DfFusionState>>;
  observe: (event: ServerEvent) => void;
  set: (node: string, state: DfFusionState) => void;
  forget: (nodes: readonly string[]) => void;
  reset: () => void;
}

export const useFusionStore = create<FusionStore>((set) => ({
  byNode: {},
  observe: (event) => {
    if (event.type !== "DfFusionUpdate") {
      return;
    }
    const { node, state: fusion } = event.data;
    set((state) => ({ byNode: { ...state.byNode, [node]: fusion } }));
  },
  set: (node, fusion) => set((state) => ({ byNode: { ...state.byNode, [node]: fusion } })),
  forget: (nodes) => set((state) => ({ byNode: omitNodes(state.byNode, nodes) })),
  reset: () => set({ byNode: {} }),
}));

export const FUSION_ACTION = "Fusion";
export const CLEAR_ACTION = "Clear";

export function useFusionSeed(node: string): void {
  const query = useQuery(fusionQuery(node));
  const held = useFusionStore((store) => store.byNode[node] !== undefined);
  const apply = useCallback(
    (state: DfFusionState) => useFusionStore.getState().set(node, state),
    [node],
  );
  useSeed(node, FUSION_ACTION, query, held, apply);
}

export function useFusionClear(node: string): { clear: () => void; pending: boolean } {
  const reset = useMutation({
    mutationFn: () => resetFusion(node),
    onSuccess: () => clearAction(node, CLEAR_ACTION),
    onError: (error) => failAction(node, CLEAR_ACTION, error),
  });
  return { clear: () => reset.mutate(), pending: reset.isPending };
}
