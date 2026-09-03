import { useMutation, useQueryClient } from "@tanstack/react-query";
import { X } from "lucide-react";
import { useState } from "react";
import { FaceBody, FaceEmpty, FaceFooter } from "../canvas/nodes/NodeShell";
import { STATE_KEY, startScan, startScanSession, stopScan, stopScanSession } from "../lib/api";
import { useScannerStore } from "../lib/scanner";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, ScanMode, ScanSession } from "../lib/types";
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
  gangCandidates,
  ganged,
  holdCandidates,
  liveStatus,
  MIN_STEP_KHZ,
  newRange,
  parseRanges,
  type RangeInput,
  scanRefusal,
  sweepKind,
  targetCount,
} from "./scanner";

const DEFAULT_THRESHOLD_DB = -55;
const DEFAULT_MARGIN_DB = 12;

export function ScannerPanel({
  active,
  empty,
  others = [],
  session = null,
}: {
  active: DeviceSet | null;
  empty: string;
  others?: readonly DeviceSet[];
  session?: ScanSession | null;
}) {
  const queryClient = useQueryClient();
  const pushed = useScannerStore((s) => (active ? s.byDeviceSet[active.id] : undefined));
  const clearLive = useScannerStore((s) => s.clear);
  const [ranges, setRanges] = useState<RangeInput[]>(() => [newRange()]);
  const [mode, setMode] = useState<ScanMode>("targets");
  const [thresholdDb, setThresholdDb] = useState(DEFAULT_THRESHOLD_DB);
  const [marginDb, setMarginDb] = useState(DEFAULT_MARGIN_DB);
  const [hardwareSweep, setHardwareSweep] = useState(true);
  const [holdChannel, setHoldChannel] = useState("");
  const [gang, setGang] = useState<readonly number[]>([]);

  const status = liveStatus(active, pushed);
  const candidates = gangCandidates(others, active);
  const partners = ganged(session, active);
  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });

  const startMut = useMutation({
    mutationFn: async (deviceSet: number) => {
      const parsed = parseRanges(ranges);
      if (typeof parsed === "string") {
        throw new Error(parsed);
      }
      const hold = holdChannel === "" ? undefined : Number(holdChannel);
      const settings = {
        mode,
        ranges: parsed.ranges,
        frequencies: [],
        threshold_db: thresholdDb,
        margin_db: marginDb,
        dwell_ms: 250,
        resume_ms: 1500,
        measure_bw_hz: 12_500,
        hardware_sweep: hardwareSweep,
        ...(hold === undefined ? {} : { hold_channel: hold }),
      };
      const joining = gang.filter((id) => candidates.some((set) => set.id === id));
      if (joining.length === 0) {
        return startScan(deviceSet, settings);
      }
      await startScanSession([deviceSet, ...joining], settings);
      return undefined;
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const stopMut = useMutation({
    mutationFn: async (deviceSet: number) => {
      if (partners.length > 0) {
        await stopScanSession();
        return;
      }
      await stopScan(deviceSet);
    },
    onSuccess: (_status, deviceSet) => {
      clearLive(deviceSet);
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const parsed = parseRanges(ranges);
  const busy = startMut.isPending || stopMut.isPending;
  const refusal = scanRefusal(active);
  const patchRange = (id: string, patch: Partial<RangeInput>): void =>
    setRanges((current) => current.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  if (active === null) {
    return (
      <FaceBody>
        <FaceEmpty>{empty}</FaceEmpty>
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
            <ReadoutRow label="Looking for">
              {status.settings.mode === "close_call"
                ? `anything ${status.settings.margin_db ?? DEFAULT_MARGIN_DB} dB over the noise`
                : "the listed frequencies"}
            </ReadoutRow>
            <ReadoutRow label="Sweep">{sweepKind(active, status)}</ReadoutRow>
            {partners.length > 0 && (
              <ReadoutRow label="Ganged with">
                {partners.length === 1 ? "1 other radio" : `${partners.length} other radios`}
              </ReadoutRow>
            )}
            <ReadoutRow label="Share">
              {formatMhz(status.first_hz)} – {formatMhz(status.last_hz)}
            </ReadoutRow>
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
                {active.capabilities.hardware_sweep === true && (
                  <SettingRow label="Firmware sweep">
                    <Checkbox
                      label="Let the radio sweep itself"
                      checked={hardwareSweep}
                      onChange={setHardwareSweep}
                    />
                  </SettingRow>
                )}
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
              {candidates.length > 0 && (
                <SettingGroup label="Also sweep with">
                  {candidates.map((set) => (
                    <SettingRow key={set.id} label={set.device.label}>
                      <Checkbox
                        label={`Include ${set.device.label} in the sweep`}
                        checked={gang.includes(set.id)}
                        onChange={(on) =>
                          setGang((current) =>
                            on ? [...current, set.id] : current.filter((id) => id !== set.id),
                          )
                        }
                      />
                    </SettingRow>
                  ))}
                </SettingGroup>
              )}
            </Settings>

            <Readout>
              <ReadoutRow label="Sweep">{sweepKind(active, null)}</ReadoutRow>
              <ReadoutRow label="Targets">
                {typeof parsed === "string" ? (
                  <span className="text-danger">{parsed}</span>
                ) : (
                  `${targetCount(parsed.ranges)} per sweep${
                    gang.length > 0 ? ` across ${gang.length + 1} radios` : ""
                  }`
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
              onClick={() => setRanges((current) => [...current, newRange()])}
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
