import { agcDelta, agcGainDb, laneAgc } from "../canvas/nodes/deviceNode";
import type { DeviceSet } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { Checkbox } from "./Checkbox";
import { formatGain } from "./capabilities";

export function agcTip(set: DeviceSet, stream: number, advised: boolean): string {
  if (!laneAgc(set, stream).on) {
    return advised ? "AGC off, as coherent lanes want" : "Let the radio set its own gain";
  }
  const db = agcGainDb(set, stream);
  const stage = set.capabilities.gains[0];
  const reading = db === null || stage === undefined ? "" : ` at ${formatGain(stage, db)} dB`;
  return `AGC on${reading}${advised ? ". Fixed gain keeps coherent lanes calibrated" : ""}`;
}

export function AgcAuto({
  set,
  stream,
  port,
  advised,
}: {
  set: DeviceSet;
  stream: number;
  port?: string;
  advised: boolean;
}) {
  const { applyPatch } = useDevicePatch();
  const agc = laneAgc(set, stream);
  return (
    <label className="flex items-center gap-1.5" title={agcTip(set, stream, advised)}>
      <Checkbox
        label={`${port === undefined ? "" : `${port} `}automatic gain`}
        checked={agc.on}
        onChange={(on) => applyPatch(set.id, agcDelta(set.capabilities, stream, { ...agc, on }))}
      />
      <span className={`legend ${agc.on && advised ? "text-warn" : ""}`}>Auto</span>
    </label>
  );
}
