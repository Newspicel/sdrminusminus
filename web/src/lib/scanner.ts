// Live scanner progress. `ScannerUpdate` bypasses TanStack Query for the same
// reason decoder frames do: a running scan retunes several times a second, and one full-state
// refetch per step would cost more than the scan. `DeviceSet.scanner` in the state snapshot is
// still the authority — this store is the fast path between snapshots, and it clears itself
// when the scan stops.
import { create } from "zustand";
import type { ScannerStatus, ServerEvent } from "./types";

/** Updates are staged and published at most this often, so a scan stepping at its dwell rate
 * cannot re-render the panel faster than a human can read it. */
export const FLUSH_MS = 150;

export interface ScannerState {
  /** Latest pushed status per device set. Absent means "no update since the last snapshot",
   * not "no scan" — the snapshot answers that. */
  byDeviceSet: Readonly<Record<number, ScannerStatus>>;
  observe: (event: ServerEvent) => void;
  /** Drop a set's live status — on scan stop, or when the set goes away. */
  clear: (deviceSet: number) => void;
  reset: () => void;
}

let pending: Record<number, ScannerStatus> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

export const useScannerStore = create<ScannerState>((set) => {
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
      if (event.type !== "ScannerUpdate") {
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
