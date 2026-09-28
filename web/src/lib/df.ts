import { create } from "zustand";
import type { DfFusionState, ServerEvent } from "./types";

export interface DfStoreState {
  byNode: Readonly<Record<string, DfFusionState>>;
  observe: (event: ServerEvent) => void;
  forget: (node: string) => void;
  reset: () => void;
}

export const useDfStore = create<DfStoreState>((set) => ({
  byNode: {},
  observe: (event: ServerEvent) => {
    if (event.type !== "DfFusionUpdate") {
      return;
    }
    const { node, state: fusion } = event.data;
    set((state) => ({ byNode: { ...state.byNode, [node]: fusion } }));
  },
  forget: (node: string) =>
    set((state) => {
      if (!(node in state.byNode)) {
        return state;
      }
      const { [node]: _dropped, ...rest } = state.byNode;
      return { byNode: rest };
    }),
  reset: () => set({ byNode: {} }),
}));
