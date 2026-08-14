import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { STATE_KEY, startScan, stopScan } from "../lib/api";
import { useScannerStore } from "../lib/scanner";
import { pushToast } from "../lib/toasts";
import type { DeviceSet } from "../lib/types";
import { LABEL } from "./controls";
import { InlineAlert } from "./InlineAlert";
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
        <div className="flex flex-col gap-1 rounded border border-border bg-muted px-3 py-2">
          <div className="flex items-baseline justify-between gap-2">
            <span
              className={`font-mono text-sm ${
                status.state === "holding" ? "text-primary" : "text-foreground"
              }`}
            >
              {status.state === "holding" ? "holding" : "scanning"} · {formatMhz(status.current_hz)}
            </span>
            <span className="font-mono text-xs text-muted-foreground">
              {formatDb(status.current_db)}
            </span>
          </div>
          <div className="font-mono text-[10px] text-muted-foreground">
            {status.targets} targets · {status.sweeps} sweeps · {status.hits} hits
            {status.settings.hold_channel != null && ` · channel ${status.settings.hold_channel}`}
          </div>
          {status.error != null && (
            <InlineAlert className="font-mono text-xs">{status.error}</InlineAlert>
          )}
        </div>
      ) : (
        <>
          {ranges.map((range, i) => (
            <div key={i} className="flex flex-wrap items-center gap-2">
              <Input
                className="w-24"
                inputMode="decimal"
                aria-label={`Range ${i + 1} start (MHz)`}
                value={range.startMhz}
                onChange={(e) => updateRange(setRanges, i, { startMhz: e.target.value })}
              />
              <span className="text-muted-foreground/70">–</span>
              <Input
                className="w-24"
                inputMode="decimal"
                aria-label={`Range ${i + 1} stop (MHz)`}
                value={range.stopMhz}
                onChange={(e) => updateRange(setRanges, i, { stopMhz: e.target.value })}
              />
              <span className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70">
                MHz · step
              </span>
              <Input
                className="w-20"
                inputMode="decimal"
                aria-label={`Range ${i + 1} step (kHz)`}
                value={range.stepKhz}
                onChange={(e) => updateRange(setRanges, i, { stepKhz: e.target.value })}
              />
              <span className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70">
                kHz
              </span>
              {ranges.length > 1 && (
                <Button
                  type="button"
                  variant="destructive"
                  size="sm"
                  aria-label={`Remove range ${i + 1}`}
                  onClick={() => setRanges(ranges.filter((_, j) => j !== i))}
                >
                  ×
                </Button>
              )}
            </div>
          ))}

          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setRanges([...ranges, DEFAULT_RANGE])}
            >
              Add range
            </Button>
            <Label className={LABEL}>
              Threshold
              <Input
                className="w-20"
                inputMode="decimal"
                aria-label="Scan threshold (dB)"
                value={thresholdDb}
                onChange={(e) => setThresholdDb(e.target.value)}
              />
              dB
            </Label>
            <Label className={LABEL}>
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
            </Label>
            <span className="font-mono text-[10px] text-muted-foreground">
              {typeof parsed === "string" ? parsed : `${count} targets`}
            </span>
          </div>
        </>
      )}

      <div className="flex gap-2">
        {running ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!active || busy}
            onClick={() => active && stopMut.mutate(active.id)}
          >
            Stop scan
          </Button>
        ) : (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!active || busy || typeof parsed === "string" || refusal !== null}
            onClick={() => active && startMut.mutate(active.id)}
          >
            Start scan
          </Button>
        )}
      </div>
      {!active && <span className="text-sm text-muted-foreground">Open a device to scan.</span>}
      {refusal !== null && <span className="text-sm text-muted-foreground">{refusal}</span>}
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
