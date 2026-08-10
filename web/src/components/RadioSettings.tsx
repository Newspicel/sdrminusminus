// Capability-driven receiver controls (PLAN §6): everything renders from `Capabilities` +
// `DeviceSettings` alone, so a new device setting needs zero frontend work. The dial and the
// tune step keep first-class UI in the top bar; the rest lives here, one row per setting, in
// the radio popover — these are consulted, not watched (DESIGN.md §5).
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { FIELD } from "./controls";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { useDebouncedCommit } from "./useDebouncedCommit";

const ROW = "grid grid-cols-[4.5rem_1fr] items-center gap-3";

export function RadioSettings({ active }: { active: DeviceSet }) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;
  const sampleRate = settings.sample_rate ?? 0;
  const rateRange = caps.sample_rate_range;

  // The device may report a bandwidth between the discrete capability points; without an option
  // for it the browser would show the first option as a lie and make it unselectable.
  const offListBandwidth =
    settings.bandwidth != null && !caps.bandwidths.includes(settings.bandwidth)
      ? settings.bandwidth
      : null;

  return (
    <div className="flex flex-col gap-2">
      <div className={ROW}>
        <span className="legend">Rate</span>
        {caps.sample_rates.length > 0 ? (
          <select
            className={`${FIELD} w-full`}
            value={sampleRate}
            aria-label="Sample rate"
            onChange={(e) => applyPatch(active.id, { sample_rate: Number(e.target.value) })}
          >
            {caps.sample_rates.map((rate) => (
              <option key={rate} value={rate}>
                {(rate / 1e6).toFixed(3)} MS/s
              </option>
            ))}
          </select>
        ) : (
          <span className="flex items-center gap-2">
            <NumberField
              label="Sample rate (MS/s)"
              value={sampleRate / 1e6}
              min={rateRange ? rateRange.min / 1e6 : undefined}
              max={rateRange ? rateRange.max / 1e6 : undefined}
              step={rateRange?.step != null ? rateRange.step / 1e6 : 0.001}
              onCommit={(msps) => applyPatch(active.id, { sample_rate: Math.round(msps * 1e6) })}
              className="w-24"
            />
            <span className="legend">MS/s</span>
          </span>
        )}
      </div>

      {caps.bandwidths.length > 0 && (
        <div className={ROW}>
          <span className="legend">Filter</span>
          <select
            className={`${FIELD} w-full`}
            value={settings.bandwidth ?? caps.bandwidths[0]}
            aria-label="Analog bandwidth"
            onChange={(e) => applyPatch(active.id, { bandwidth: Number(e.target.value) })}
          >
            {offListBandwidth != null && (
              <option value={offListBandwidth}>{formatHz(offListBandwidth)} (current)</option>
            )}
            {caps.bandwidths.map((bandwidth) => (
              <option key={bandwidth} value={bandwidth}>
                {formatHz(bandwidth)}
              </option>
            ))}
          </select>
        </div>
      )}

      {caps.antennas.length > 1 && (
        <div className={ROW}>
          <span className="legend">Antenna</span>
          <select
            className={`${FIELD} w-full`}
            value={settings.antenna ?? caps.antennas[0]}
            aria-label="Antenna"
            onChange={(e) => applyPatch(active.id, { antenna: e.target.value })}
          >
            {caps.antennas.map((antenna) => (
              <option key={antenna} value={antenna}>
                {antenna}
              </option>
            ))}
          </select>
        </div>
      )}

      {caps.gains.map((stage) => (
        <GainControl
          key={stage.name}
          stage={stage}
          value={settings.gains?.find((g) => g.stage === stage.name)?.value_db ?? stage.range.min}
          onCommit={(db) => applyPatch(active.id, { gains: [{ stage: stage.name, value_db: db }] })}
        />
      ))}

      <div className={ROW}>
        <span className="legend">PPM</span>
        <NumberField
          label="Frequency correction (ppm)"
          value={settings.ppm ?? 0}
          step={1}
          onCommit={(ppm) => applyPatch(active.id, { ppm })}
          className="w-20"
        />
      </div>

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
    <div className={ROW}>
      <span className="legend">{stage.name}</span>
      <span className="flex items-center gap-2">
        <input
          type="range"
          className="min-w-0 flex-1 accent-accent"
          min={stage.range.min}
          max={stage.range.max}
          step={stage.range.step ?? 0.1}
          value={shown}
          onChange={(e) => change(Number(e.target.value))}
          aria-label={`${stage.name} gain (dB)`}
        />
        <span className="w-14 shrink-0 text-right font-mono text-xs text-ink">
          {shown.toFixed(1)} <span className="text-ink-faint">dB</span>
        </span>
      </span>
    </div>
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
        <div className={ROW}>
          <span className="legend">{setting.name}</span>
          <input
            type="checkbox"
            className="size-4 accent-accent justify-self-start"
            aria-label={setting.name}
            checked={typeof raw === "boolean" ? raw : setting.default}
            onChange={(e) => onCommit(e.target.checked)}
          />
        </div>
      );
    case "enum":
      return (
        <div className={ROW}>
          <span className="legend">{setting.name}</span>
          <select
            className={`${FIELD} w-full`}
            aria-label={setting.name}
            value={typeof raw === "string" ? raw : setting.default}
            onChange={(e) => onCommit(e.target.value)}
          >
            {setting.options.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        </div>
      );
    case "range":
      return (
        <div className={ROW}>
          <span className="legend">{setting.name}</span>
          <span className="flex items-center gap-2">
            <NumberField
              label={`${setting.name} (${setting.unit})`}
              value={typeof raw === "number" ? raw : setting.range.min}
              min={setting.range.min}
              max={setting.range.max}
              step={setting.range.step ?? undefined}
              onCommit={onCommit}
              className="w-24"
            />
            <span className="legend">{setting.unit}</span>
          </span>
        </div>
      );
  }
}
