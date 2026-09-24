import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useLayoutEffect, useRef } from "react";
import { followOffset } from "../components/converter";
import { patchDevice, STATE_KEY } from "./api";
import { recordEvent } from "./diagnostics";
import { toastError } from "./toasts";
import type { DeviceSettings, StateSnapshot, StreamScope, StreamSettings } from "./types";

export function mergeSettings(current: DeviceSettings, delta: DeviceSettings): DeviceSettings {
  const next: DeviceSettings = { ...current };
  if (delta.center_hz != null) {
    next.center_hz = delta.center_hz;
  }
  if (delta.tuning != null) {
    next.tuning = delta.tuning;
  }
  if (delta.sample_rate != null) {
    next.sample_rate = delta.sample_rate;
  }
  if (delta.ppm != null) {
    next.ppm = delta.ppm;
  }
  if (delta.offset_hz != null) {
    next.offset_hz = delta.offset_hz;
  }
  if (delta.antenna != null) {
    next.antenna = delta.antenna;
  }
  if (delta.bandwidth != null) {
    next.bandwidth = delta.bandwidth;
  }
  if (delta.dc_block != null) {
    next.dc_block = delta.dc_block;
  }
  if (delta.bias_tee != null) {
    next.bias_tee = delta.bias_tee;
  }
  if (delta.agc != null) {
    next.agc = delta.agc;
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
      ...(entry.tuning != null ? { tuning: entry.tuning } : {}),
      ...(entry.agc != null ? { agc: entry.agc } : {}),
      ...(entry.antenna != null ? { antenna: entry.antenna } : {}),
      ...(entry.gains ? { gains: mergeByKey(existing.gains, entry.gains, (g) => g.stage) } : {}),
    };
  }
  return merged;
}

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
  if (scope.tuning === true && overrides.tuning != null) {
    resolved.tuning = overrides.tuning;
  }
  if (scope.gain === true && overrides.gains) {
    resolved.gains = mergeByKey(resolved.gains, overrides.gains, (g) => g.stage);
  }
  if (scope.antenna === true && overrides.antenna != null) {
    resolved.antenna = overrides.antenna;
  }
  if (scope.agc === true && overrides.agc != null) {
    resolved.agc = overrides.agc;
  }
  return resolved;
}

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

export function createPatchQueue(
  send: (ds: number, settings: DeviceSettings) => Promise<unknown>,
): (ds: number, delta: DeviceSettings) => void {
  const waiting = new Map<number, DeviceSettings>();
  const inFlight = new Set<number>();

  const run = (ds: number): void => {
    const settings = waiting.get(ds);
    if (settings === undefined) {
      inFlight.delete(ds);
      return;
    }
    waiting.delete(ds);
    inFlight.add(ds);
    void send(ds, settings)
      .catch(() => undefined)
      .finally(() => run(ds));
  };

  return (ds: number, delta: DeviceSettings): void => {
    const queued = waiting.get(ds);
    waiting.set(ds, queued === undefined ? delta : mergeSettings(queued, delta));
    if (!inFlight.has(ds)) {
      run(ds);
    }
  };
}

export function refusedByRadio(error: unknown): error is Error {
  return error instanceof Error && (error as { code?: unknown }).code === "engine";
}

export function useDevicePatch(): {
  applyPatch: (ds: number, delta: DeviceSettings) => void;
  cachedSettings: (ds: number) => DeviceSettings | undefined;
} {
  const queryClient = useQueryClient();
  const patchMut = useMutation({
    mutationFn: (v: { ds: number; settings: DeviceSettings }) => patchDevice(v.ds, v.settings),
    onError: (error) => {
      if (refusedByRadio(error)) {
        recordEvent("error", "device", error.message);
        return;
      }
      toastError(error);
    },
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const mutate = useRef(patchMut.mutateAsync);
  useLayoutEffect(() => {
    mutate.current = patchMut.mutateAsync;
  });
  const queue = useRef<((ds: number, delta: DeviceSettings) => void) | null>(null);
  queue.current ??= createPatchQueue((ds, settings) => mutate.current({ ds, settings }));

  const applyPatch = (ds: number, delta: DeviceSettings): void => {
    void queryClient.cancelQueries({ queryKey: STATE_KEY });
    const prev = queryClient.getQueryData<StateSnapshot>(STATE_KEY);
    const current = prev?.device_sets.find((d) => d.id === ds);
    if (!prev || current === undefined) {
      return;
    }
    const followed = followOffset(current.settings, delta);
    queryClient.setQueryData<StateSnapshot>(STATE_KEY, {
      ...prev,
      device_sets: prev.device_sets.map((d) =>
        d.id === ds ? { ...d, settings: mergeSettings(d.settings, followed) } : d,
      ),
    });
    queue.current?.(ds, followed);
  };

  const cachedSettings = (ds: number): DeviceSettings | undefined =>
    queryClient.getQueryData<StateSnapshot>(STATE_KEY)?.device_sets.find((d) => d.id === ds)
      ?.settings;

  return { applyPatch, cachedSettings };
}
