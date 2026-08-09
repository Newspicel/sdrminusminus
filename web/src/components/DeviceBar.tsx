// Device open/close + tuning (PLAN §5, §10). Well-known settings (frequency, rate) get
// first-class controls; mutations rely on the WS `StateChanged` event to refresh state.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createDeviceSet, deleteDeviceSet, devicesQuery, patchDevice, STATE_KEY } from "../lib/api";
import type { DeviceSet, DeviceSettings, StateSnapshot } from "../lib/types";
import { FrequencyReadout } from "./FrequencyReadout";

const TUNE_STEPS_HZ = [-1_000_000, -100_000, 100_000, 1_000_000];

const BTN =
  "rounded border border-line bg-panel-2 px-2.5 py-1 text-sm text-ink transition-colors " +
  "hover:border-accent hover:text-accent disabled:opacity-40";

export function DeviceBar({
  active,
  onSelect,
}: {
  active: DeviceSet | null;
  onSelect: (ds: number | null) => void;
}) {
  const queryClient = useQueryClient();
  const devices = useQuery(devicesQuery());
  const createMut = useMutation({ mutationFn: createDeviceSet, onSuccess: onSelect });
  const deleteMut = useMutation({
    mutationFn: deleteDeviceSet,
    onSuccess: () => onSelect(null),
  });
  // On error, refetch the authoritative snapshot to undo the optimistic write below.
  const patchMut = useMutation({
    mutationFn: (v: { ds: number; settings: DeviceSettings }) => patchDevice(v.ds, v.settings),
    onError: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  // Apply an absolute settings patch with a *synchronous* optimistic cache update so rapid
  // clicks accumulate (each reads the previous click's result) instead of all sending the same
  // stale target. The server applies center_hz absolutely, so the optimistic value matches the
  // eventual WS-refreshed state — no flicker.
  const applyPatch = (dsId: number, patch: DeviceSettings) => {
    const prev = queryClient.getQueryData<StateSnapshot>(STATE_KEY);
    if (prev) {
      queryClient.setQueryData<StateSnapshot>(STATE_KEY, {
        ...prev,
        device_sets: prev.device_sets.map((d) =>
          d.id === dsId ? { ...d, settings: { ...d.settings, ...patch } } : d,
        ),
      });
    }
    patchMut.mutate({ ds: dsId, settings: patch });
  };

  const cachedCenterHz = (dsId: number, fallback: number): number =>
    queryClient.getQueryData<StateSnapshot>(STATE_KEY)?.device_sets.find((d) => d.id === dsId)
      ?.settings.center_hz ?? fallback;

  if (!active) {
    const found = devices.data?.devices ?? [];
    return (
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-sm text-ink-dim">No device open.</span>
        {found.map((d) => (
          <button
            key={`${d.driver}:${d.key}`}
            type="button"
            className={BTN}
            disabled={createMut.isPending}
            onClick={() => createMut.mutate(`${d.driver}:${d.key}`)}
          >
            Open {d.label}
          </button>
        ))}
      </div>
    );
  }

  const centerHz = active.settings.center_hz ?? 0;
  const sampleRate = active.settings.sample_rate ?? 0;

  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
      <FrequencyReadout hz={centerHz} />

      <div className="flex gap-1">
        {TUNE_STEPS_HZ.map((step) => (
          <button
            key={step}
            type="button"
            className={`${BTN} font-mono tabular-nums`}
            onClick={() =>
              applyPatch(active.id, { center_hz: cachedCenterHz(active.id, centerHz) + step })
            }
          >
            {step > 0 ? "+" : "−"}
            {Math.abs(step) / 1000}k
          </button>
        ))}
      </div>

      <label className="flex items-center gap-2 text-sm text-ink-dim">
        Rate
        <select
          className="rounded border border-line bg-panel-2 px-2 py-1 font-mono text-ink"
          value={sampleRate}
          onChange={(e) => applyPatch(active.id, { sample_rate: Number(e.target.value) })}
        >
          {active.capabilities.sample_rates.map((r) => (
            <option key={r} value={r}>
              {(r / 1e6).toFixed(3)} MS/s
            </option>
          ))}
        </select>
      </label>

      <span className="text-xs text-ink-dim">
        {active.device.label} · {active.status}
      </span>

      <button
        type="button"
        className={`${BTN} ml-auto hover:border-danger hover:text-danger`}
        onClick={() => deleteMut.mutate(active.id)}
      >
        Close
      </button>
    </div>
  );
}
