// Device open/close + tuning (PLAN §5, §10). Well-known settings (frequency, rate) get
// first-class controls; mutations rely on the WS `StateChanged` event to refresh state.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  createDeviceSet,
  deleteDeviceSet,
  devicesQuery,
  RECORDINGS_KEY,
  recordDeviceSet,
  STATE_KEY,
} from "../lib/api";
import type { DeviceSet, RecordAction, RecordingStatus } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { BTN, FIELD } from "./controls";
import { FrequencyReadout } from "./FrequencyReadout";
import { NumberField } from "./NumberField";
import { deriveRecordControl, formatBytes, formatDuration, recordingElapsedS } from "./recordings";

const TUNE_STEPS_HZ = [-1_000_000, -100_000, 100_000, 1_000_000];

export function DeviceBar({
  active,
  onSelect,
}: {
  active: DeviceSet | null;
  onSelect: (ds: number | null) => void;
}) {
  const queryClient = useQueryClient();
  const devices = useQuery(devicesQuery());
  const { applyPatch, cachedSettings, patchError, dismissPatchError } = useDevicePatch();
  // A failed open/close must be visible (CLAUDE.md: no silent failure) — the WS state event
  // never fires for a set that was never created, so nothing else can surface it.
  const [mutError, setMutError] = useState<string | null>(null);
  const createMut = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: (id) => {
      setMutError(null);
      onSelect(id);
    },
    onError: (e) => setMutError(e.message),
  });
  const deleteMut = useMutation({
    mutationFn: deleteDeviceSet,
    onSuccess: () => {
      setMutError(null);
      onSelect(null);
    },
    onError: (e) => setMutError(e.message),
  });
  // Belt-and-braces like the panels': the server emits "recordings" (stop indexes a SigMF pair)
  // and "device_set" (the state this control derives from), but a missed WS event must not
  // strand the record button.
  const recordMut = useMutation({
    mutationFn: (v: { ds: number; action: RecordAction }) => recordDeviceSet(v.ds, v.action),
    onSuccess: () => setMutError(null),
    onError: (e) => setMutError(e.message),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
    },
  });

  if (!active) {
    const found = devices.data?.devices ?? [];
    return (
      <div className="flex flex-col gap-2">
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
        {mutError !== null && (
          <MutationErrorBanner message={mutError} onDismiss={() => setMutError(null)} />
        )}
      </div>
    );
  }

  const centerHz = active.settings.center_hz ?? 0;
  const sampleRate = active.settings.sample_rate ?? 0;
  const rateRange = active.capabilities.sample_rate_range;
  const record = deriveRecordControl(active);

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

        {record.kind === "recording" ? (
          <span className="flex items-center gap-2">
            <RecordingReadout status={record.status} sampleRate={sampleRate} />
            <button
              type="button"
              className={BTN}
              disabled={recordMut.isPending}
              onClick={() => recordMut.mutate({ ds: active.id, action: "stop" })}
            >
              ■ Stop
            </button>
          </span>
        ) : (
          <button
            type="button"
            className={BTN}
            disabled={!record.canStart || recordMut.isPending}
            onClick={() => recordMut.mutate({ ds: active.id, action: "start" })}
          >
            ● Rec
          </button>
        )}

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

      {record.kind === "recording" && record.status.error != null && (
        <div
          role="alert"
          className="rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          recording fault · {record.status.error}
        </div>
      )}

      {mutError !== null && (
        <MutationErrorBanner message={mutError} onDismiss={() => setMutError(null)} />
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

// Child component so the 1 s elapsed tick re-renders only the readout, not the whole bar.
// Once the recording faulted the writer is dead, so the ticker stops — wall clock would
// overstate what was captured.
function RecordingReadout({ status, sampleRate }: { status: RecordingStatus; sampleRate: number }) {
  const faulted = status.error != null;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (faulted) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [faulted]);
  return (
    <span className="font-mono text-xs tabular-nums text-danger">
      ● {formatDuration(recordingElapsedS(status, now, sampleRate))} · {formatBytes(status.bytes)}
      {status.overruns > 0 && ` · ${status.overruns} overruns`}
    </span>
  );
}

function MutationErrorBanner({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  return (
    <div
      role="alert"
      className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
    >
      <span>Rejected: {message}</span>
      <button type="button" className="shrink-0 underline" onClick={onDismiss}>
        dismiss
      </button>
    </div>
  );
}
