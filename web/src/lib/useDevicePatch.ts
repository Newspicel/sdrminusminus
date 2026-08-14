// The one optimistic PATCH pipeline for device settings (). The cache is updated
// *synchronously* with the server's merge semantics, so rapid edits accumulate — each reads the
// previous edit's result instead of re-sending a stale target — and the WS-refreshed state then
// matches the optimistic value, no flicker. Once a mutation settles the authoritative snapshot
// is refetched, so the cache always converges to the server's state.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { patchDevice, STATE_KEY } from "./api";
import { pushToast } from "./toasts";
import type { DeviceSettings, StateSnapshot, StreamScope, StreamSettings } from "./types";

// Mirrors `DeviceSettings::merge_from` (crates/wire): present scalars overwrite; gains/extra
// merge per stage/name, so a delta carrying one stage must not clobber the others. Stream
// overrides merge by stream index and each entry's gains per stage name again — so a radio-wide
// retune never wipes a lane's override, and one lane's dial never drags the others.
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
  if (delta.streams) {
    next.streams = mergeStreams(current.streams, delta.streams);
  }
  return next;
}

function mergeStreams(
  current: StreamSettings[] | undefined,
  delta: readonly StreamSettings[],
): StreamSettings[] {
  const merged = [...(current ?? [])];
  for (const entry of delta) {
    const at = merged.findIndex((existing) => existing.stream === entry.stream);
    const existing = merged[at];
    if (existing === undefined) {
      merged.push(entry);
      continue;
    }
    merged[at] = {
      ...existing,
      ...(entry.center_hz != null ? { center_hz: entry.center_hz } : {}),
      ...(entry.antenna != null ? { antenna: entry.antenna } : {}),
      ...(entry.gains ? { gains: mergeByKey(existing.gains, entry.gains, (g) => g.stage) } : {}),
    };
  }
  return merged;
}

/**
 * Mirrors `DeviceSettings::for_stream` (crates/wire), the single resolution point: what stream
 * `index` is actually set to — its own override where the radio declares that setting
 * per-stream, the radio-wide value otherwise. The result carries no `streams` of its own; it is
 * one lane's resolved view, not the overrides table.
 */
export function forStream(
  settings: DeviceSettings,
  index: number,
  scope: StreamScope | undefined,
): DeviceSettings {
  const { streams, ...resolved } = settings;
  const overrides = streams?.find((entry) => entry.stream === index);
  if (overrides === undefined || scope === undefined) {
    return resolved;
  }
  if (scope.tuning === true && overrides.center_hz != null) {
    resolved.center_hz = overrides.center_hz;
  }
  if (scope.gain === true && overrides.gains) {
    resolved.gains = mergeByKey(resolved.gains, overrides.gains, (g) => g.stage);
  }
  if (scope.antenna === true && overrides.antenna != null) {
    resolved.antenna = overrides.antenna;
  }
  return resolved;
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

export function useDevicePatch(): {
  applyPatch: (ds: number, delta: DeviceSettings) => void;
  cachedSettings: (ds: number) => DeviceSettings | undefined;
} {
  const queryClient = useQueryClient();
  const patchMut = useMutation({
    mutationFn: (v: { ds: number; settings: DeviceSettings }) => patchDevice(v.ds, v.settings),
    // A rejected PATCH must be visible, not just snap the control back (CLAUDE.md: no silent
    // failure). The toast stack is shared, so a rejection from any panel is reported once.
    onError: (error) => pushToast(error.message),
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

  return { applyPatch, cachedSettings };
}
