import { agcDelta, agcGainDb, laneAgc } from "../canvas/nodes/deviceNode";
import type { DeviceSet } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { Button } from "./BaseControls";
import { agcStageIndex, formatGain } from "./capabilities";
import { TOGGLE_QUIET } from "./controls";
import { Tip } from "./Tip";

export function agcTip(set: DeviceSet, stream: number, advised: boolean): string {
  if (!laneAgc(set, stream).on) {
    return advised ? "AGC off, as coherent lanes want" : "Let the radio set its own gain";
  }
  const db = agcGainDb(set, stream);
  const stage = set.capabilities.gains[agcStageIndex(set.capabilities.gains)];
  const reading = db === null || stage === undefined ? "" : ` at ${formatGain(stage, db)} dB`;
  return `AGC on${reading}${advised ? ". Fixed gain keeps coherent lanes calibrated" : ""}`;
}

export function AutoToggle({
  label,
  pressed,
  title,
  warn = false,
  disabled = false,
  onChange,
}: {
  label: string;
  pressed: boolean | "mixed";
  title: string;
  warn?: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <Tip
      text={title}
      render={
        <Button
          type="button"
          className={`${TOGGLE_QUIET} ${warn ? "text-warn" : ""}`}
          aria-label={label}
          aria-pressed={pressed}
          disabled={disabled}
          onClick={() => onChange(pressed !== true)}
        />
      }
    >
      Auto
    </Tip>
  );
}

export function AgcAuto({
  set,
  stream,
  port,
  advised,
  heldBy,
}: {
  set: DeviceSet;
  stream: number;
  port?: string;
  advised: boolean;
  heldBy?: string;
}) {
  const { applyPatch } = useDevicePatch();
  const agc = laneAgc(set, stream);
  return (
    <AutoToggle
      label={`${port === undefined ? "" : `${port} `}automatic gain`}
      pressed={agc.on}
      title={heldBy === undefined ? agcTip(set, stream, advised) : `Set on ${heldBy}`}
      warn={agc.on && advised}
      disabled={heldBy !== undefined}
      onChange={(on) => applyPatch(set.id, agcDelta(set.capabilities, stream, { ...agc, on }))}
    />
  );
}
