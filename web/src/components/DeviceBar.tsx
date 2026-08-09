// Device open/close + tuning (PLAN §5, §10). Well-known settings (frequency, rate) get
// first-class controls; mutations rely on the WS `StateChanged` event to refresh state.
import { useMutation, useQuery } from "@tanstack/react-query";
import { createDeviceSet, deleteDeviceSet, devicesQuery } from "../lib/api";
import type { DeviceSet } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { FrequencyReadout } from "./FrequencyReadout";
import { FIELD, NumberField } from "./NumberField";

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
  const devices = useQuery(devicesQuery());
  const { applyPatch, cachedSettings, patchError, dismissPatchError } = useDevicePatch();
  const createMut = useMutation({ mutationFn: createDeviceSet, onSuccess: onSelect });
  const deleteMut = useMutation({
    mutationFn: deleteDeviceSet,
    onSuccess: () => onSelect(null),
  });

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
  const rateRange = active.capabilities.sample_rate_range;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
        <FrequencyReadout hz={centerHz} />

        <div className="flex gap-1">
          {TUNE_STEPS_HZ.map((step) => (
            <button
              key={step}
              type="button"
              className={`${BTN} font-mono tabular-nums`}
              onClick={() =>
                applyPatch(active.id, {
                  center_hz: (cachedSettings(active.id)?.center_hz ?? centerHz) + step,
                })
              }
            >
              {step > 0 ? "+" : "−"}
              {Math.abs(step) / 1000}k
            </button>
          ))}
        </div>

        <label className="flex items-center gap-2 text-sm text-ink-dim">
          Rate
          {active.capabilities.sample_rates.length > 0 ? (
            <select
              className={FIELD}
              value={sampleRate}
              onChange={(e) => applyPatch(active.id, { sample_rate: Number(e.target.value) })}
            >
              {active.capabilities.sample_rates.map((r) => (
                <option key={r} value={r}>
                  {(r / 1e6).toFixed(3)} MS/s
                </option>
              ))}
            </select>
          ) : (
            <>
              <NumberField
                label="Sample rate (MS/s)"
                value={sampleRate / 1e6}
                min={rateRange ? rateRange.min / 1e6 : undefined}
                max={rateRange ? rateRange.max / 1e6 : undefined}
                step={rateRange?.step != null ? rateRange.step / 1e6 : 0.001}
                onCommit={(msps) => applyPatch(active.id, { sample_rate: Math.round(msps * 1e6) })}
                className="w-24"
              />
              MS/s
            </>
          )}
        </label>

        <span className="text-xs text-ink-dim">
          {active.device.label} ·{" "}
          <span className={active.status === "error" ? "text-danger" : undefined}>
            {active.status}
          </span>
        </span>

        <button
          type="button"
          className={`${BTN} ml-auto hover:border-danger hover:text-danger`}
          onClick={() => deleteMut.mutate(active.id)}
        >
          Close
        </button>
      </div>

      {active.status === "error" && (
        <div
          role="alert"
          className="rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          device fault · {active.error ?? "unknown error"}
        </div>
      )}

      {patchError !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Rejected: {patchError}</span>
          <button type="button" className="shrink-0 underline" onClick={dismissPatchError}>
            dismiss
          </button>
        </div>
      )}
    </div>
  );
}
