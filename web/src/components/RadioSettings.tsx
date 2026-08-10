// Capability-driven receiver controls (PLAN §6): everything renders from `Capabilities` +
// `DeviceSettings` alone, so a new device setting needs zero frontend work. The dial and the
// tune step keep first-class UI in the top bar; the rest lives here, one row per setting, in
// the radio popover — these are consulted, not watched (DESIGN.md §5).
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { Checkbox } from "./Checkbox";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { Select, withCurrent } from "./Select";
import { Slider } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

const ROW = "grid grid-cols-[4.5rem_1fr] items-center gap-3";

const formatMsps = (hz: number): string => `${(hz / 1e6).toFixed(3)} MS/s`;

export function RadioSettings({ active }: { active: DeviceSet }) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;
  const sampleRate = settings.sample_rate ?? 0;
  const rateRange = caps.sample_rate_range;
  const bandwidth = settings.bandwidth ?? caps.bandwidths[0] ?? 0;

  return (
    <div className="flex flex-col gap-2">
      <div className={ROW}>
        <span className="legend">Rate</span>
        {caps.sample_rates.length > 0 ? (
          <Select
            label="Sample rate"
            className="w-full"
            value={sampleRate}
            options={withCurrent(
              sampleRate,
              caps.sample_rates.map((rate) => ({ value: rate, label: formatMsps(rate) })),
              formatMsps,
            )}
            onChange={(sample_rate) => applyPatch(active.id, { sample_rate })}
          />
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
          <Select
            label="Analog bandwidth"
            className="w-full"
            value={bandwidth}
            options={withCurrent(
              bandwidth,
              caps.bandwidths.map((hz) => ({ value: hz, label: formatHz(hz) })),
              formatHz,
            )}
            onChange={(hz) => applyPatch(active.id, { bandwidth: hz })}
          />
        </div>
      )}

      {caps.antennas.length > 1 && (
        <div className={ROW}>
          <span className="legend">Antenna</span>
          <Select
            label="Antenna"
            className="w-full"
            value={settings.antenna ?? caps.antennas[0] ?? ""}
            options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
            onChange={(antenna) => applyPatch(active.id, { antenna })}
          />
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
        <Slider
          label={`${stage.name} gain (dB)`}
          className="min-w-0 flex-1"
          min={stage.range.min}
          max={stage.range.max}
          step={stage.range.step ?? 0.1}
          value={shown}
          onChange={change}
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
          <span className="justify-self-start">
            <Checkbox
              label={setting.name}
              checked={typeof raw === "boolean" ? raw : setting.default}
              onChange={onCommit}
            />
          </span>
        </div>
      );
    case "enum":
      return (
        <div className={ROW}>
          <span className="legend">{setting.name}</span>
          <Select
            label={setting.name}
            className="w-full"
            value={typeof raw === "string" ? raw : setting.default}
            options={setting.options.map((option) => ({ value: option, label: option }))}
            onChange={onCommit}
          />
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
