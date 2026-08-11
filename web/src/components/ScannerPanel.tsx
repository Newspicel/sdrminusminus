// Frequency scanner controls (PLAN §13 P2, M5). Live progress comes from the `ScannerUpdate`
// WS event via the scanner store, never from polling; the state snapshot is what says whether
// a scan exists at all.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { STATE_KEY, startScan, stopScan } from "../lib/api";
import { useScannerStore } from "../lib/scanner";
import { pushToast } from "../lib/toasts";
import type { DeviceSet } from "../lib/types";
import { BTN, FIELD, LABEL } from "./controls";
import { Select } from "./Select";
import {
  DEFAULT_RANGE,
  formatDb,
  formatMhz,
  holdCandidates,
  liveStatus,
  parseRanges,
  type RangeInput,
  scanRefusal,
  targetCount,
} from "./scanner";

export function ScannerPanel({ active }: { active: DeviceSet | null }) {
  const queryClient = useQueryClient();
  const pushed = useScannerStore((s) => (active ? s.byDeviceSet[active.id] : undefined));
  const clearLive = useScannerStore((s) => s.clear);
  const [ranges, setRanges] = useState<RangeInput[]>([DEFAULT_RANGE]);
  const [thresholdDb, setThresholdDb] = useState("-55");
  const [holdChannel, setHoldChannel] = useState<string>("");

  const status = liveStatus(active, pushed);
  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });

  const startMut = useMutation({
    mutationFn: async (ds: number) => {
      const parsed = parseRanges(ranges);
      if (typeof parsed === "string") {
        throw new Error(parsed);
      }
      const threshold = Number(thresholdDb);
      if (!Number.isFinite(threshold)) {
        throw new Error("the threshold must be a number in dB");
      }
      const hold = holdChannel === "" ? undefined : Number(holdChannel);
      return startScan(ds, {
        ranges: parsed.ranges,
        frequencies: [],
        threshold_db: threshold,
        dwell_ms: 250,
        resume_ms: 1500,
        measure_bw_hz: 12_500,
        ...(hold === undefined ? {} : { hold_channel: hold }),
      });
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const stopMut = useMutation({
    mutationFn: stopScan,
    onSuccess: (_status, ds) => {
      // The pushed status outlives the scan; drop it or the panel keeps showing the last step.
      clearLive(ds);
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const parsed = parseRanges(ranges);
  const count = typeof parsed === "string" ? 0 : targetCount(parsed.ranges);
  const running = status !== null;
  const busy = startMut.isPending || stopMut.isPending;
  const refusal = scanRefusal(active);

  return (
    <div className="flex flex-col gap-2 p-3">
      {running ? (
        <div className="flex flex-col gap-1 rounded border border-line bg-panel-2 px-3 py-2">
          <div className="flex items-baseline justify-between gap-2">
            <span
              className={`font-mono text-sm ${
                status.state === "holding" ? "text-accent" : "text-ink"
              }`}
            >
              {status.state === "holding" ? "holding" : "scanning"} · {formatMhz(status.current_hz)}
            </span>
            <span className="font-mono text-xs text-ink-dim">{formatDb(status.current_db)}</span>
          </div>
          <div className="font-mono text-[10px] text-ink-dim">
            {status.targets} targets · {status.sweeps} sweeps · {status.hits} hits
            {status.settings.hold_channel != null && ` · channel ${status.settings.hold_channel}`}
          </div>
          {status.error != null && (
            <div role="alert" className="font-mono text-xs text-danger">
              {status.error}
            </div>
          )}
        </div>
      ) : (
        <>
          {ranges.map((range, i) => (
            // The editor rows have no identity of their own; the index is the identity.
            // biome-ignore lint/suspicious/noArrayIndexKey: rows are positional by definition
            <div key={i} className="flex flex-wrap items-center gap-2">
              <input
                className={`${FIELD} w-24`}
                inputMode="decimal"
                aria-label={`Range ${i + 1} start (MHz)`}
                value={range.startMhz}
                onChange={(e) => updateRange(setRanges, i, { startMhz: e.target.value })}
              />
              <span className="text-ink-faint">–</span>
              <input
                className={`${FIELD} w-24`}
                inputMode="decimal"
                aria-label={`Range ${i + 1} stop (MHz)`}
                value={range.stopMhz}
                onChange={(e) => updateRange(setRanges, i, { stopMhz: e.target.value })}
              />
              <span className="legend">MHz · step</span>
              <input
                className={`${FIELD} w-20`}
                inputMode="decimal"
                aria-label={`Range ${i + 1} step (kHz)`}
                value={range.stepKhz}
                onChange={(e) => updateRange(setRanges, i, { stepKhz: e.target.value })}
              />
              <span className="legend">kHz</span>
              {ranges.length > 1 && (
                <button
                  type="button"
                  className={`${BTN} hover:border-danger hover:text-danger`}
                  aria-label={`Remove range ${i + 1}`}
                  onClick={() => setRanges(ranges.filter((_, j) => j !== i))}
                >
                  ×
                </button>
              )}
            </div>
          ))}

          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              className={BTN}
              onClick={() => setRanges([...ranges, DEFAULT_RANGE])}
            >
              Add range
            </button>
            <label className={LABEL}>
              Threshold
              <input
                className={`${FIELD} w-20`}
                inputMode="decimal"
                aria-label="Scan threshold (dB)"
                value={thresholdDb}
                onChange={(e) => setThresholdDb(e.target.value)}
              />
              dB
            </label>
            <label className={LABEL}>
              Listen on
              <Select
                label="Hold channel"
                value={holdChannel}
                options={[
                  { value: "", label: "nothing" },
                  ...holdCandidates(active).map((c) => ({
                    value: String(c.id),
                    label: `channel ${c.id} (${c.settings.params.type})`,
                  })),
                ]}
                onChange={setHoldChannel}
              />
            </label>
            <span className="font-mono text-[10px] text-ink-dim">
              {typeof parsed === "string" ? parsed : `${count} targets`}
            </span>
          </div>
        </>
      )}

      <div className="flex gap-2">
        {running ? (
          <button
            type="button"
            className={BTN}
            disabled={!active || busy}
            onClick={() => active && stopMut.mutate(active.id)}
          >
            Stop scan
          </button>
        ) : (
          <button
            type="button"
            className={BTN}
            disabled={!active || busy || typeof parsed === "string" || refusal !== null}
            onClick={() => active && startMut.mutate(active.id)}
          >
            Start scan
          </button>
        )}
      </div>
      {!active && <span className="text-sm text-ink-dim">Open a device to scan.</span>}
      {refusal !== null && <span className="text-sm text-ink-dim">{refusal}</span>}
    </div>
  );
}

function updateRange(
  setRanges: React.Dispatch<React.SetStateAction<RangeInput[]>>,
  index: number,
  patch: Partial<RangeInput>,
): void {
  setRanges((current) => current.map((r, i) => (i === index ? { ...r, ...patch } : r)));
}
