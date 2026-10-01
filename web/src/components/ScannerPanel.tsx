import { useMutation, useQueryClient } from "@tanstack/react-query";
import { X } from "lucide-react";
import { useState } from "react";
import { FaceBody, FaceFooter } from "../canvas/nodes/NodeShell";
import { STATE_KEY, skipScan, startScan, stopScan } from "../lib/api";
import { decoderKey, useScannerStore } from "../lib/scanner";
import { pushToast } from "../lib/toasts";
import type { ChannelInfo, DeviceSet, ScanMode, ScannerStatus } from "../lib/types";
import { Button } from "./BaseControls";
import { BTN, BTN_DANGER, BTN_PRIMARY, BTN_SM } from "./controls";
import { ChipField, Chips, ChoiceChip, NumberChip, SettingChip, ToggleChip } from "./face/Chips";
import { FaceFault } from "./face/Fault";
import { Readout, Readouts } from "./face/Readouts";
import { FaceStats, Stat } from "./face/Stats";
import { Icon } from "./Icon";
import { NumberField } from "./NumberField";
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

const SCAN_MODES = [
  { value: "targets", label: "Listed", title: "The listed frequencies" },
  { value: "close_call", label: "Strongest", title: "The strongest signal near me" },
] as const;

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
  const pushed = useScannerStore((s) =>
    active && channel ? s.byDecoder[decoderKey(active.id, channel.id)] : undefined,
  );
  const clearLive = useScannerStore((s) => s.clear);
  const [ranges, setRanges] = useState<RangeInput[]>(() => [newRange()]);
  const [mode, setMode] = useState<ScanMode>("targets");
  const [thresholdDb, setThresholdDb] = useState(DEFAULT_THRESHOLD_DB);
  const [marginDb, setMarginDb] = useState(DEFAULT_MARGIN_DB);
  const [hardwareSweep, setHardwareSweep] = useState(true);

  const status = liveStatus(active, channel?.id ?? null, pushed);
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
      return startScan(target, settings);
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const stopMut = useMutation({
    mutationFn: stopScan,
    onSuccess: (_status, decoder) => {
      clearLive(decoder.deviceSet, decoder.channel);
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
  return (
    <>
      <FaceBody title={active === null ? hint : undefined}>
        {status !== null ? (
          <>
            <Readouts ruled={false}>
              <Readout label="State">
                <span className={holding ? "text-accent" : ""}>
                  {holding ? "holding" : "scanning"}
                </span>
              </Readout>
              <Readout label="Frequency">{formatMhz(status.current_hz)}</Readout>
              <Readout label="Looking for">
                {status.settings.mode === "close_call"
                  ? `anything ${status.settings.margin_db ?? DEFAULT_MARGIN_DB} dB over the noise`
                  : "the listed frequencies"}
              </Readout>
              <Readout label="Sweep">{sweepKind(active, status)}</Readout>
              <Readout label="Span">
                {formatMhz(status.first_hz)} to {formatMhz(status.last_hz)}
              </Readout>
              <Readout label="Level">{formatDb(status.current_db)}</Readout>
            </Readouts>
            {status.error != null && <FaceFault message={status.error} />}
          </>
        ) : (
          <>
            <Readouts ruled={false}>
              {channel !== null && (
                <Readout label="Feeds">
                  {channel.settings.params.type} at {formatMhz(channel.settings.frequency_hz)}
                </Readout>
              )}
              <Readout label="Sweep">{sweepKind(active, null)}</Readout>
            </Readouts>
            <ScanSetup
              ranges={ranges}
              onRanges={setRanges}
              mode={mode}
              onMode={setMode}
              thresholdDb={thresholdDb}
              onThreshold={setThresholdDb}
              marginDb={marginDb}
              onMargin={setMarginDb}
              firmwareSweep={active?.capabilities.hardware_sweep === true}
              hardwareSweep={hardwareSweep}
              onHardwareSweep={setHardwareSweep}
            />
            {typeof parsed === "string" && <FaceFault message={parsed} />}
          </>
        )}
      </FaceBody>

      <FaceFooter>
        {status !== null ? (
          <ScanStats status={status} />
        ) : (
          typeof parsed !== "string" && (
            <FaceStats>
              <Stat label="Targets" title="Frequencies per sweep">
                {targetCount(parsed.ranges)}
              </Stat>
            </FaceStats>
          )
        )}
        {status !== null && active !== null && channel !== null ? (
          <>
            <Button
              type="button"
              className={BTN}
              disabled={busy || !holding}
              title="Leave this frequency and never hold on it again this scan"
              onClick={() => skipMut.mutate({ deviceSet: active.id, channel: channel.id })}
            >
              Skip
            </Button>
            <Button
              type="button"
              className={BTN_DANGER}
              disabled={busy}
              onClick={() => stopMut.mutate({ deviceSet: active.id, channel: channel.id })}
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

function ScanStats({ status }: { status: ScannerStatus }) {
  const skipped = status.settings.skip?.length ?? 0;
  return (
    <FaceStats>
      <Stat label="Targets" title="Frequencies per sweep">
        {status.targets}
      </Stat>
      <Stat label="Sweeps" title="Sweeps finished">
        {status.sweeps}
      </Stat>
      <Stat label="Hits" title="Signals held">
        {status.hits}
      </Stat>
      {skipped > 0 && (
        <Stat label="Skipped" title="Frequencies skipped this scan">
          {skipped}
        </Stat>
      )}
    </FaceStats>
  );
}

function RangeChip({
  range,
  index,
  numbered,
  onPatch,
  onRemove,
}: {
  range: RangeInput;
  index: number;
  numbered: boolean;
  onPatch: (patch: Partial<RangeInput>) => void;
  onRemove?: () => void;
}) {
  const name = `Range ${index + 1}`;
  const reversed = range.stopMhz < range.startMhz;
  return (
    <SettingChip
      label={numbered ? name : "Range"}
      value={`${range.startMhz}-${range.stopMhz}`}
      unit="MHz"
      title={`${name}, every ${range.stepKhz} kHz`}
      tone={reversed ? "danger" : undefined}
    >
      {(close) => (
        <div className="flex flex-col gap-2">
          <ChipField label="From">
            <NumberField
              className="min-w-0 flex-1"
              label={`${name} start`}
              value={range.startMhz}
              min={0}
              step={0.1}
              onCommit={(startMhz) => onPatch({ startMhz })}
              unit="MHz"
            />
          </ChipField>
          <ChipField label="To">
            <NumberField
              className="min-w-0 flex-1"
              label={`${name} stop`}
              value={range.stopMhz}
              min={0}
              step={0.1}
              invalid={reversed}
              onCommit={(stopMhz) => onPatch({ stopMhz })}
              unit="MHz"
            />
          </ChipField>
          <ChipField label="Step">
            <NumberField
              className="min-w-0 flex-1"
              label={`${name} step`}
              value={range.stepKhz}
              min={MIN_STEP_KHZ}
              step={MIN_STEP_KHZ}
              onCommit={(stepKhz) => onPatch({ stepKhz })}
              unit="kHz"
            />
          </ChipField>
          {onRemove !== undefined && (
            <Button
              type="button"
              className={`${BTN_SM} self-start hover:text-danger`}
              aria-label={`Remove range ${index + 1}`}
              onClick={() => {
                onRemove();
                close();
              }}
            >
              <Icon glyph={X} size={12} />
              Remove
            </Button>
          )}
        </div>
      )}
    </SettingChip>
  );
}

function ScanSetup({
  ranges,
  onRanges,
  mode,
  onMode,
  thresholdDb,
  onThreshold,
  marginDb,
  onMargin,
  firmwareSweep,
  hardwareSweep,
  onHardwareSweep,
}: {
  ranges: readonly RangeInput[];
  onRanges: (update: (current: RangeInput[]) => RangeInput[]) => void;
  mode: ScanMode;
  onMode: (mode: ScanMode) => void;
  thresholdDb: number;
  onThreshold: (db: number) => void;
  marginDb: number;
  onMargin: (db: number) => void;
  firmwareSweep: boolean;
  hardwareSweep: boolean;
  onHardwareSweep: (on: boolean) => void;
}) {
  const patchRange = (id: string, patch: Partial<RangeInput>): void =>
    onRanges((current) => current.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  return (
    <Chips className="border-t border-line p-2">
      {ranges.map((range, index) => (
        <RangeChip
          key={range.id}
          range={range}
          index={index}
          numbered={ranges.length > 1}
          onPatch={(patch) => patchRange(range.id, patch)}
          onRemove={
            ranges.length > 1
              ? () => onRanges((current) => current.filter((other) => other.id !== range.id))
              : undefined
          }
        />
      ))}
      <ChoiceChip
        label="Find"
        title="Scan mode"
        value={mode}
        options={SCAN_MODES}
        onChange={onMode}
      />
      {mode === "close_call" ? (
        <NumberChip
          label="Over noise"
          title="Close call margin"
          unit="dB"
          value={marginDb}
          min={1}
          max={60}
          step={1}
          onCommit={onMargin}
        />
      ) : (
        <NumberChip
          label="Threshold"
          title="Scan threshold"
          unit="dB"
          value={thresholdDb}
          min={-120}
          max={0}
          step={1}
          onCommit={onThreshold}
        />
      )}
      {firmwareSweep && (
        <ToggleChip
          label="Firmware sweep"
          title="Let the radio sweep itself"
          on={hardwareSweep}
          onChange={onHardwareSweep}
        />
      )}
    </Chips>
  );
}
