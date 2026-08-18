import { create } from "zustand";
import type { HuntStatus, ServerEvent } from "./types";

export const FLUSH_MS = 40;

export interface HuntState {
  byDeviceSet: Readonly<Record<number, HuntStatus>>;
  observe: (event: ServerEvent) => void;
  clear: (deviceSet: number) => void;
  reset: () => void;
}

let pending: Record<number, HuntStatus> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

export const useHuntStore = create<HuntState>((set) => {
  const flush = () => {
    timer = null;
    const staged = pending;
    pending = null;
    if (staged === null) {
      return;
    }
    set((state) => ({ byDeviceSet: { ...state.byDeviceSet, ...staged } }));
  };

  return {
    byDeviceSet: {},
    observe: (event: ServerEvent) => {
      if (event.type !== "HuntUpdate") {
        return;
      }
      pending = { ...pending, [event.data.device_set]: event.data.status };
      if (timer === null) {
        timer = setTimeout(flush, FLUSH_MS);
      }
    },
    clear: (deviceSet: number) => {
      if (pending !== null) {
        delete pending[deviceSet];
      }
      set((state) => {
        if (!(deviceSet in state.byDeviceSet)) {
          return state;
        }
        const { [deviceSet]: _dropped, ...rest } = state.byDeviceSet;
        return { byDeviceSet: rest };
      });
    },
    reset: () => {
      pending = null;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      set({ byDeviceSet: {} });
    },
  };
});
