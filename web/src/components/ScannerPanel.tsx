import { useMutation, useQueryClient } from "@tanstack/react-query";
import { X } from "lucide-react";
import { useState } from "react";
import { FaceBody, FaceFooter } from "../canvas/nodes/NodeShell";
import { STATE_KEY, skipScan, startScan, stopScan } from "../lib/api";
import { useScannerStore } from "../lib/scanner";
import { pushToast } from "../lib/toasts";
import type { ChannelInfo, DeviceSet, ScanMode } from "../lib/types";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN, BTN_DANGER, BTN_PRIMARY, ICON_BTN_SM } from "./controls";
import { Icon } from "./Icon";
import { NumberField } from "./NumberField";
import { Readout, ReadoutRow } from "./Readout";
import { Select } from "./Select";
import { SettingGroup, SettingRow, Settings } from "./Settings";
import {
  formatDb,
  formatMhz,
  liveStatus,
  MIN_STEP_KHZ,
  newRange,
  parseRanges,
  type RangeInput,
  sweepKind,
  targetCount,
} from "./scanner";

const DEFAULT_THRESHOLD_DB = -55;
const DEFAULT_MARGIN_DB = 12;

export function ScannerPanel({
  active,
  channel,
  hint,
}: {
  active: DeviceSet | null;
  channel: ChannelInfo | null;
  hint: string;
}) {
  const queryClient = useQueryClient();
  const pushed = useScannerStore((s) => (active ? s.byDeviceSet[active.id] : undefined));
  const clearLive = useScannerStore((s) => s.clear);
  const [ranges, setRanges] = useState<RangeInput[]>(() => [newRange()]);
  const [mode, setMode] = useState<ScanMode>("targets");
  const [thresholdDb, setThresholdDb] = useState(DEFAULT_THRESHOLD_DB);
  const [marginDb, setMarginDb] = useState(DEFAULT_MARGIN_DB);
  const [hardwareSweep, setHardwareSweep] = useState(true);

  const status = liveStatus(active, pushed);
  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });

  const startMut = useMutation({
    mutationFn: async (target: { deviceSet: number; channel: number }) => {
      const parsed = parseRanges(ranges);
      if (typeof parsed === "string") {
        throw new Error(parsed);
      }
      const settings = {
        channel: target.channel,
        mode,
        ranges: parsed.ranges,
        frequencies: [],
        threshold_db: thresholdDb,
        margin_db: marginDb,
        dwell_ms: 250,
        resume_ms: 1500,
        hardware_sweep: hardwareSweep,
      };
      return startScan(target.deviceSet, settings);
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const stopMut = useMutation({
    mutationFn: stopScan,
    onSuccess: (_status, deviceSet) => {
      clearLive(deviceSet);
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const skipMut = useMutation({
    mutationFn: skipScan,
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const parsed = parseRanges(ranges);
  const busy = startMut.isPending || stopMut.isPending || skipMut.isPending;
  const holding = status?.state === "holding";
  const patchRange = (id: string, patch: Partial<RangeInput>): void =>
    setRanges((current) => current.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  return (
    <>
      <FaceBody title={active === null ? hint : undefined}>
        {status !== null ? (
          <Readout separated={false}>
            <ReadoutRow label="State">
              <span className={status.state === "holding" ? "text-accent" : ""}>
                {status.state === "holding" ? "holding" : "scanning"}
              </span>
            </ReadoutRow>
            <ReadoutRow label="Frequency">{formatMhz(status.current_hz)}</ReadoutRow>
            <ReadoutRow label="Looking for">
              {status.settings.mode === "close_call"
                ? `anything ${status.settings.margin_db ?? DEFAULT_MARGIN_DB} dB over the noise`
                : "the listed frequencies"}
            </ReadoutRow>
            <ReadoutRow label="Sweep">{sweepKind(active, status)}</ReadoutRow>
            <ReadoutRow label="Span">
              {formatMhz(status.first_hz)} to {formatMhz(status.last_hz)}
            </ReadoutRow>
            <ReadoutRow label="Level">{formatDb(status.current_db)}</ReadoutRow>
            <ReadoutRow label="Targets">{status.targets}</ReadoutRow>
            <ReadoutRow label="Sweeps">{status.sweeps}</ReadoutRow>
            <ReadoutRow label="Hits">{status.hits}</ReadoutRow>
            {(status.settings.skip?.length ?? 0) > 0 && (
              <ReadoutRow label="Skipped">{status.settings.skip?.length}</ReadoutRow>
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
                  key={range.id}
                  label={ranges.length > 1 ? `Range ${index + 1}` : "Range"}
                  action={
                    ranges.length > 1 && (
                      <Button
                        type="button"
                        className={`${ICON_BTN_SM} hover:text-danger`}
                        aria-label={`Remove range ${index + 1}`}
                        onClick={() =>
                          setRanges((current) => current.filter((other) => other.id !== range.id))
                        }
                      >
                        <Icon glyph={X} size={12} />
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
                      onCommit={(startMhz) => patchRange(range.id, { startMhz })}
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
                      onCommit={(stopMhz) => patchRange(range.id, { stopMhz })}
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
                      onCommit={(stepKhz) => patchRange(range.id, { stepKhz })}
                      className="w-24"
                    />
                    <span className="legend">kHz</span>
                  </SettingRow>
                </SettingGroup>
              ))}

              <SettingGroup label="Sweep">
                <SettingRow label="Looking for">
                  <Select
                    label="Scan mode"
                    value={mode}
                    options={[
                      { value: "targets", label: "the listed frequencies" },
                      { value: "close_call", label: "the strongest signal near me" },
                    ]}
                    onChange={setMode}
                  />
                </SettingRow>
                {mode === "close_call" ? (
                  <SettingRow label="Over noise">
                    <NumberField
                      label="Close call margin (dB)"
                      value={marginDb}
                      min={1}
                      max={60}
                      step={1}
                      onCommit={setMarginDb}
                      className="w-24"
                    />
                    <span className="legend">dB</span>
                  </SettingRow>
                ) : (
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
                )}
                {active?.capabilities.hardware_sweep === true && (
                  <SettingRow label="Firmware sweep">
                    <Checkbox
                      label="Let the radio sweep itself"
                      checked={hardwareSweep}
                      onChange={setHardwareSweep}
                    />
                  </SettingRow>
                )}
              </SettingGroup>
            </Settings>

            <Readout>
              {channel !== null && (
                <ReadoutRow label="Feeds">
                  {channel.settings.params.type} at {formatMhz(channel.settings.frequency_hz)}
                </ReadoutRow>
              )}
              <ReadoutRow label="Sweep">{sweepKind(active, null)}</ReadoutRow>
              <ReadoutRow label="Targets">
                {typeof parsed === "string" ? (
                  <span className="text-danger">{parsed}</span>
                ) : (
                  `${targetCount(parsed.ranges)} per sweep`
                )}
              </ReadoutRow>
            </Readout>
          </>
        )}
      </FaceBody>

      <FaceFooter>
        {status !== null && active !== null ? (
          <>
            <Button
              type="button"
              className={BTN}
              disabled={busy || !holding}
              title="Leave this frequency and never hold on it again this scan"
              onClick={() => skipMut.mutate(active.id)}
            >
              Skip
            </Button>
            <Button
              type="button"
              className={BTN_DANGER}
              disabled={busy}
              onClick={() => stopMut.mutate(active.id)}
            >
              Stop scan
            </Button>
          </>
        ) : (
          <>
            <Button
              type="button"
              className={BTN}
              onClick={() => setRanges((current) => [...current, newRange()])}
            >
              Add range
            </Button>
            <Button
              type="button"
              className={BTN_PRIMARY}
              disabled={active === null || channel === null || busy || typeof parsed === "string"}
              onClick={() =>
                active !== null &&
                channel !== null &&
                startMut.mutate({ deviceSet: active.id, channel: channel.id })
              }
            >
              Start scan
            </Button>
          </>
        )}
      </FaceFooter>
    </>
  );
}
