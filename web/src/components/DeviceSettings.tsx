// Capability-driven device controls (PLAN §6): everything renders from `Capabilities` +
// `DeviceSettings` alone, so a new device setting needs zero frontend work. Well-known settings
// (frequency, rate) keep first-class UI in `DeviceBar`; this strip carries the rest generically.
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { FIELD } from "./controls";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { useDebouncedCommit } from "./useDebouncedCommit";

const LABEL = "flex items-center gap-2 text-sm text-ink-dim";

export function DeviceSettingsPanel({ active }: { active: DeviceSet }) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;

  // The device may report a bandwidth between the discrete capability points; without an option
  // for it the browser would show the first option as a lie and make it unselectable.
  const offListBandwidth =
    settings.bandwidth != null && !caps.bandwidths.includes(settings.bandwidth)
      ? settings.bandwidth
      : null;

  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-2 border-b border-line bg-panel px-4 py-2">
      {caps.gains.map((stage) => (
        <GainControl
          key={stage.name}
          stage={stage}
          value={settings.gains?.find((g) => g.stage === stage.name)?.value_db ?? stage.range.min}
          onCommit={(db) => applyPatch(active.id, { gains: [{ stage: stage.name, value_db: db }] })}
        />
      ))}

      {caps.antennas.length > 1 && (
        <label className={LABEL}>
          Antenna
          <select
            className={FIELD}
            value={settings.antenna ?? caps.antennas[0]}
            onChange={(e) => applyPatch(active.id, { antenna: e.target.value })}
          >
            {caps.antennas.map((antenna) => (
              <option key={antenna} value={antenna}>
                {antenna}
              </option>
            ))}
          </select>
        </label>
      )}

      <label className={LABEL}>
        PPM
        <NumberField
          label="Frequency correction (ppm)"
          value={settings.ppm ?? 0}
          step={1}
          onCommit={(ppm) => applyPatch(active.id, { ppm })}
          className="w-16"
        />
      </label>

      {caps.bandwidths.length > 0 && (
        <label className={LABEL}>
          BW
          <select
            className={FIELD}
            value={settings.bandwidth ?? caps.bandwidths[0]}
            onChange={(e) => applyPatch(active.id, { bandwidth: Number(e.target.value) })}
          >
            {offListBandwidth != null && (
              <option value={offListBandwidth}>{formatHz(offListBandwidth)} (current)</option>
            )}
            {caps.bandwidths.map((bw) => (
              <option key={bw} value={bw}>
                {formatHz(bw)}
              </option>
            ))}
          </select>
        </label>
      )}

      {(caps.extra ?? []).map((setting) => (
        <ExtraControl
          key={setting.name}
          setting={setting}
          raw={settings.extra?.find((e) => e.name === setting.name)?.value}
          onCommit={(value) => applyPatch(active.id, { extra: [{ name: setting.name, value }] })}
        />
      ))}
    </div>
  );
}

function GainControl({
  stage,
  value,
  onCommit,
}: {
  stage: GainStage;
  value: number;
  onCommit: (db: number) => void;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  return (
    <label className={LABEL}>
      {stage.name}
      <input
        type="range"
        className="w-28 accent-accent"
        min={stage.range.min}
        max={stage.range.max}
        step={stage.range.step ?? 0.1}
        value={shown}
        onChange={(e) => change(Number(e.target.value))}
        aria-label={`${stage.name} gain (dB)`}
      />
      <span className="w-16 text-right font-mono tabular-nums text-ink">
        {shown.toFixed(1)} <span className="text-ink-dim">dB</span>
      </span>
    </label>
  );
}

function ExtraControl({
  setting,
  raw,
  onCommit,
}: {
  setting: ExtraSetting;
  raw: unknown;
  onCommit: (value: boolean | string | number) => void;
}) {
  switch (setting.kind) {
    case "bool":
      return (
        <label className={LABEL}>
          <input
            type="checkbox"
            className="accent-accent"
            checked={typeof raw === "boolean" ? raw : setting.default}
            onChange={(e) => onCommit(e.target.checked)}
          />
          {setting.name}
        </label>
      );
    case "enum":
      return (
        <label className={LABEL}>
          {setting.name}
          <select
            className={FIELD}
            value={typeof raw === "string" ? raw : setting.default}
            onChange={(e) => onCommit(e.target.value)}
          >
            {setting.options.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        </label>
      );
    case "range":
      return (
        <label className={LABEL}>
          {setting.name}
          <NumberField
            label={`${setting.name} (${setting.unit})`}
            value={typeof raw === "number" ? raw : setting.range.min}
            min={setting.range.min}
            max={setting.range.max}
            step={setting.range.step ?? undefined}
            onCommit={onCommit}
          />
          {setting.unit}
        </label>
      );
  }
}
