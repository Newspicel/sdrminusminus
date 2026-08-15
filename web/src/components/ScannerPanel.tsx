import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { FaceBody, FaceEmpty, FaceFooter } from "../canvas/nodes/NodeShell";
import { STATE_KEY, startScan, stopScan } from "../lib/api";
import { useScannerStore } from "../lib/scanner";
import { pushToast } from "../lib/toasts";
import type { DeviceSet } from "../lib/types";
import { Button } from "./BaseControls";
import { BTN, BTN_DANGER, BTN_PRIMARY, ICON_BTN_SM } from "./controls";
import { NumberField } from "./NumberField";
import { Readout, ReadoutRow } from "./Readout";
import { Select } from "./Select";
import { SettingGroup, SettingRow, Settings } from "./Settings";
import {
  DEFAULT_RANGE,
  formatDb,
  formatMhz,
  holdCandidates,
  liveStatus,
  MIN_STEP_KHZ,
  parseRanges,
  type RangeInput,
  scanRefusal,
  targetCount,
} from "./scanner";

const DEFAULT_THRESHOLD_DB = -55;

export function ScannerPanel({ active }: { active: DeviceSet | null }) {
  const queryClient = useQueryClient();
  const pushed = useScannerStore((s) => (active ? s.byDeviceSet[active.id] : undefined));
  const clearLive = useScannerStore((s) => s.clear);
  const [ranges, setRanges] = useState<RangeInput[]>([DEFAULT_RANGE]);
  const [thresholdDb, setThresholdDb] = useState(DEFAULT_THRESHOLD_DB);
  const [holdChannel, setHoldChannel] = useState("");

  const status = liveStatus(active, pushed);
  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });

  const startMut = useMutation({
    mutationFn: async (deviceSet: number) => {
      const parsed = parseRanges(ranges);
      if (typeof parsed === "string") {
        throw new Error(parsed);
      }
      const hold = holdChannel === "" ? undefined : Number(holdChannel);
      return startScan(deviceSet, {
        ranges: parsed.ranges,
        frequencies: [],
        threshold_db: thresholdDb,
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
    onSuccess: (_status, deviceSet) => {
      // The pushed status outlives the scan; drop it or the panel keeps showing the last step.
      clearLive(deviceSet);
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const parsed = parseRanges(ranges);
  const busy = startMut.isPending || stopMut.isPending;
  const refusal = scanRefusal(active);
  const patchRange = (index: number, patch: Partial<RangeInput>): void =>
    setRanges((current) => current.map((r, i) => (i === index ? { ...r, ...patch } : r)));

  if (active === null) {
    return (
      <FaceBody>
        <FaceEmpty>
          Wire this out to a device; the scanner then drives that radio's tuning.
        </FaceEmpty>
      </FaceBody>
    );
  }

  return (
    <>
      <FaceBody>
        {status !== null ? (
          <Readout separated={false}>
            <ReadoutRow label="State">
              <span className={status.state === "holding" ? "text-accent" : ""}>
                {status.state === "holding" ? "holding" : "scanning"}
              </span>
            </ReadoutRow>
            <ReadoutRow label="Frequency">{formatMhz(status.current_hz)}</ReadoutRow>
            <ReadoutRow label="Level">{formatDb(status.current_db)}</ReadoutRow>
            <ReadoutRow label="Targets">{status.targets}</ReadoutRow>
            <ReadoutRow label="Sweeps">{status.sweeps}</ReadoutRow>
            <ReadoutRow label="Hits">{status.hits}</ReadoutRow>
            {status.settings.hold_channel != null && (
              <ReadoutRow label="Listening on">channel {status.settings.hold_channel}</ReadoutRow>
            )}
            {status.error != null && (
              <ReadoutRow label="Fault">
                <span className="text-danger">{status.error}</span>
              </ReadoutRow>
            )}
          </Readout>
        ) : (
          <>
            <Settings className="p-2">
              {ranges.map((range, index) => (
                <SettingGroup
                  // The rows are a position in the list, and there is nothing else stable to key
                  // them by — two ranges may legitimately hold the same three numbers.
                  key={index}
                  label={ranges.length > 1 ? `Range ${index + 1}` : "Range"}
                  action={
                    ranges.length > 1 && (
                      <Button
                        type="button"
                        className={`${ICON_BTN_SM} hover:text-danger`}
                        aria-label={`Remove range ${index + 1}`}
                        onClick={() => setRanges(ranges.filter((_, other) => other !== index))}
                      >
                        ✕
                      </Button>
                    )
                  }
                >
                  <SettingRow label="From">
                    <NumberField
                      label={`Range ${index + 1} start (MHz)`}
                      value={range.startMhz}
                      min={0}
                      step={0.1}
                      onCommit={(startMhz) => patchRange(index, { startMhz })}
                      className="w-24"
                    />
                    <span className="legend">MHz</span>
                  </SettingRow>
                  <SettingRow label="To">
                    <NumberField
                      label={`Range ${index + 1} stop (MHz)`}
                      value={range.stopMhz}
                      min={0}
                      step={0.1}
                      invalid={range.stopMhz < range.startMhz}
                      onCommit={(stopMhz) => patchRange(index, { stopMhz })}
                      className="w-24"
                    />
                    <span className="legend">MHz</span>
                  </SettingRow>
                  <SettingRow label="Step">
                    <NumberField
                      label={`Range ${index + 1} step (kHz)`}
                      value={range.stepKhz}
                      min={MIN_STEP_KHZ}
                      step={MIN_STEP_KHZ}
                      onCommit={(stepKhz) => patchRange(index, { stepKhz })}
                      className="w-24"
                    />
                    <span className="legend">kHz</span>
                  </SettingRow>
                </SettingGroup>
              ))}

              <SettingGroup label="Sweep">
                <SettingRow label="Threshold">
                  <NumberField
                    label="Scan threshold (dB)"
                    value={thresholdDb}
                    min={-120}
                    max={0}
                    step={1}
                    onCommit={setThresholdDb}
                    className="w-24"
                  />
                  <span className="legend">dB</span>
                </SettingRow>
                <SettingRow label="Listen on">
                  <Select
                    label="Hold channel"
                    value={holdChannel}
                    options={[
                      { value: "", label: "nothing" },
                      ...holdCandidates(active).map((channel) => ({
                        value: String(channel.id),
                        label: `channel ${channel.id} (${channel.settings.params.type})`,
                      })),
                    ]}
                    onChange={setHoldChannel}
                  />
                </SettingRow>
              </SettingGroup>
            </Settings>

            <Readout>
              <ReadoutRow label="Targets">
                {typeof parsed === "string" ? (
                  <span className="text-danger">{parsed}</span>
                ) : (
                  `${targetCount(parsed.ranges)} per sweep`
                )}
              </ReadoutRow>
              {refusal !== null && (
                <ReadoutRow label="Refused">
                  <span className="text-danger">{refusal}</span>
                </ReadoutRow>
              )}
            </Readout>
          </>
        )}
      </FaceBody>

      <FaceFooter>
        {status !== null ? (
          <Button
            type="button"
            className={BTN_DANGER}
            disabled={busy}
            onClick={() => stopMut.mutate(active.id)}
          >
            Stop scan
          </Button>
        ) : (
          <>
            <Button
              type="button"
              className={BTN}
              onClick={() => setRanges([...ranges, DEFAULT_RANGE])}
            >
              Add range
            </Button>
            <Button
              type="button"
              className={BTN_PRIMARY}
              disabled={busy || typeof parsed === "string" || refusal !== null}
              onClick={() => startMut.mutate(active.id)}
            >
              Start scan
            </Button>
          </>
        )}
      </FaceFooter>
    </>
  );
}
