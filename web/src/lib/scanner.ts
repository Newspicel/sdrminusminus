import { create } from "zustand";
import type { ScannerStatus, ServerEvent } from "./types";

export function decoderKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

export const FLUSH_MS = 150;

export interface ScannerState {
  byDecoder: Readonly<Record<string, ScannerStatus>>;
  observe: (event: ServerEvent) => void;
  clear: (deviceSet: number, channel: number) => void;
  reset: () => void;
}

let pending: Record<string, ScannerStatus> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

export const useScannerStore = create<ScannerState>((set) => {
  const flush = () => {
    timer = null;
    const staged = pending;
    pending = null;
    if (staged === null) {
      return;
    }
    set((state) => ({ byDecoder: { ...state.byDecoder, ...staged } }));
  };

  return {
    byDecoder: {},
    observe: (event: ServerEvent) => {
      if (event.type !== "ScannerUpdate") {
        return;
      }
      pending = {
        ...pending,
        [decoderKey(event.data.device_set, event.data.status.settings.channel)]: event.data.status,
      };
      if (timer === null) {
        timer = setTimeout(flush, FLUSH_MS);
      }
    },
    clear: (deviceSet: number, channel: number) => {
      const key = decoderKey(deviceSet, channel);
      if (pending !== null) {
        delete pending[key];
      }
      set((state) => {
        if (!(key in state.byDecoder)) {
          return state;
        }
        const { [key]: _dropped, ...rest } = state.byDecoder;
        return { byDecoder: rest };
      });
    },
    reset: () => {
      pending = null;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      set({ byDecoder: {} });
    },
  };
});
