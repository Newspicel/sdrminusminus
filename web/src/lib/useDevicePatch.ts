// The one optimistic PATCH pipeline for device settings (PLAN §5). The cache is updated
// *synchronously* with the server's merge semantics, so rapid edits accumulate — each reads the
// previous edit's result instead of re-sending a stale target — and the WS-refreshed state then
// matches the optimistic value, no flicker. Once a mutation settles the authoritative snapshot
// is refetched, so the cache always converges to the server's state.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { create } from "zustand";
import { patchDevice, STATE_KEY } from "./api";
import type { DeviceSettings, StateSnapshot } from "./types";

// Mirrors `DeviceSettings::merge_from` (crates/wire): present scalars overwrite; gains/extra
// merge per stage/name, so a delta carrying one stage must not clobber the others.
export function mergeSettings(current: DeviceSettings, delta: DeviceSettings): DeviceSettings {
  const next: DeviceSettings = { ...current };
  if (delta.center_hz != null) {
    next.center_hz = delta.center_hz;
  }
  if (delta.sample_rate != null) {
    next.sample_rate = delta.sample_rate;
  }
  if (delta.ppm != null) {
    next.ppm = delta.ppm;
  }
  if (delta.antenna != null) {
    next.antenna = delta.antenna;
  }
  if (delta.bandwidth != null) {
    next.bandwidth = delta.bandwidth;
  }
  if (delta.gains) {
    next.gains = mergeByKey(current.gains, delta.gains, (g) => g.stage);
  }
  if (delta.extra) {
    next.extra = mergeByKey(current.extra, delta.extra, (e) => e.name);
  }
  return next;
}

/**
 * A debounce-flushed patch can outlive its device set (Close during a slider drag). A patch
 * whose target is gone is meaningless: it must be dropped, not sent and then surfaced as a
 * stale "Rejected" banner over whatever device the user opens next.
 */
export function patchTargetExists(snapshot: StateSnapshot | undefined, ds: number): boolean {
  return snapshot?.device_sets.some((d) => d.id === ds) ?? false;
}

function mergeByKey<T>(current: T[] | undefined, delta: T[], key: (item: T) => string): T[] {
  const merged = [...(current ?? [])];
  for (const item of delta) {
    const at = merged.findIndex((existing) => key(existing) === key(item));
    if (at >= 0) {
      merged[at] = item;
    } else {
      merged.push(item);
    }
  }
  return merged;
}

// Shared across hook instances so a patch rejected from any panel reaches the one banner in
// `DeviceBar` — per-instance mutation state would hide errors from sibling components.
const usePatchErrorStore = create<{
  patchError: string | null;
  setPatchError: (error: string | null) => void;
}>((set) => ({
  patchError: null,
  setPatchError: (patchError) => set({ patchError }),
}));

export function useDevicePatch(): {
  applyPatch: (ds: number, delta: DeviceSettings) => void;
  cachedSettings: (ds: number) => DeviceSettings | undefined;
  patchError: string | null;
  dismissPatchError: () => void;
} {
  const queryClient = useQueryClient();
  const patchError = usePatchErrorStore((s) => s.patchError);
  const setPatchError = usePatchErrorStore((s) => s.setPatchError);
  const patchMut = useMutation({
    mutationFn: (v: { ds: number; settings: DeviceSettings }) => patchDevice(v.ds, v.settings),
    onSuccess: () => setPatchError(null),
    // A rejected PATCH must be visible, not just snap the control back (CLAUDE.md: no silent
    // failure).
    onError: (error) => setPatchError(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const applyPatch = (ds: number, delta: DeviceSettings): void => {
    // A refetch started by an earlier StateChanged could resolve after this write and clobber
    // it — cancel in-flight fetches before touching the cache (TanStack optimistic contract).
    void queryClient.cancelQueries({ queryKey: STATE_KEY });
    const prev = queryClient.getQueryData<StateSnapshot>(STATE_KEY);
    if (!prev || !patchTargetExists(prev, ds)) {
      return;
    }
    queryClient.setQueryData<StateSnapshot>(STATE_KEY, {
      ...prev,
      device_sets: prev.device_sets.map((d) =>
        d.id === ds ? { ...d, settings: mergeSettings(d.settings, delta) } : d,
      ),
    });
    patchMut.mutate({ ds, settings: delta });
  };

  // Reads the optimistic cache, not component props, so chained edits see prior results.
  const cachedSettings = (ds: number): DeviceSettings | undefined =>
    queryClient.getQueryData<StateSnapshot>(STATE_KEY)?.device_sets.find((d) => d.id === ds)
      ?.settings;

  return { applyPatch, cachedSettings, patchError, dismissPatchError: () => setPatchError(null) };
}
