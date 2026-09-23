import { useState } from "react";
import { rxStreamCount, streamLabel } from "../canvas/graph";
import type { Capabilities, DeviceSet, ExtraSetting, GainStage, Range } from "../lib/types";
import { forStream, useDevicePatch } from "../lib/useDevicePatch";
import { Input } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import {
  AUTO_FILTER,
  agcOffered,
  agcState,
  automaticGainIsOn,
  dcBlockOn,
  filterHz,
  filterIsAuto,
  fitsSlider,
  formatGain,
  gainLabel,
  gainUnit,
  hasDcArtifact,
  hasFilter,
  isSwitch,
  manualFilter,
  settingIndex,
  snapToRanges,
  snapToStage,
  spanOf,
  stageSettings,
} from "./capabilities";
import { FIELD } from "./controls";
import { isTunable, tuningRange } from "./dial";
import { formatHz, formatSampleRate } from "./format";
import { NumberField } from "./NumberField";
import { LOOP_SETTING } from "./playback";
import { SearchableSelect } from "./SearchableSelect";
import { Select } from "./Select";
import { SettingGroup, SettingRow, Settings } from "./Settings";
import { Slider } from "./Slider";
import { withCurrent } from "./selectOptions";
import { settingLabel } from "./settingLabel";
import { useDebouncedCommit } from "./useDebouncedCommit";

const AGC_HINT = "The radio is setting this. Turn AGC off to set it by hand";

const SEARCHABLE_FROM = 12;

const READOUT = "w-14 shrink-0 text-right font-mono text-xs text-ink";

export function RadioSettings({
  active,
  className,
  sampleRateLocked = false,
}: {
  active: DeviceSet;
  className?: string;
  sampleRateLocked?: boolean;
}) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;
  const extras = (caps.extra ?? []).filter(
    (setting) => active.playback == null || setting.name !== LOOP_SETTING,
  );
  const scope = caps.per_stream;
  const streamedAntenna = scope?.antenna === true && caps.antennas.length > 1;
  const streamedGain = scope?.gain === true && caps.gains.length > 0;
  const automaticGain = automaticGainIsOn(caps, settings);
  const streams =
    streamedAntenna || streamedGain
      ? Array.from({ length: rxStreamCount(caps) }, (_, index) => index)
      : [];
  const patch = (delta: Parameters<typeof applyPatch>[1]): void => applyPatch(active.id, delta);

  return (
    <Settings className={className}>
      <SettingRow label="Rate">
        <RateControl
          caps={caps}
          sampleRate={settings.sample_rate ?? 0}
          locked={sampleRateLocked}
          onCommit={(sample_rate) => patch({ sample_rate })}
        />
      </SettingRow>

      {hasFilter(caps) && (
        <SettingRow label="Filter" title="Analog bandwidth before the ADC">
          <FilterControl active={active} onCommit={(bandwidth) => patch({ bandwidth })} />
        </SettingRow>
      )}

      {caps.antennas.length > 1 && !streamedAntenna && (
        <SettingRow label="Antenna">
          <Select
            label="Antenna"
            value={settings.antenna ?? caps.antennas[0] ?? ""}
            options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
            onChange={(antenna) => patch({ antenna })}
          />
        </SettingRow>
      )}

      {agcOffered(caps) && (
        <SettingRow label="AGC" title="The radio sets its own gain">
          <AgcControl active={active} onCommit={(agc) => patch({ agc })} />
        </SettingRow>
      )}

      {!streamedGain &&
        caps.gains.map((stage) => (
          <GainControl
            key={stage.name}
            stage={stage}
            disabled={automaticGain}
            value={settings.gains?.find((g) => g.stage === stage.name)?.value_db ?? stage.range.min}
            onCommit={(db) => patch({ gains: [{ stage: stage.name, value_db: db }] })}
          />
        ))}

      {streams.map((stream) => {
        const port = streamLabel("iq", stream, streams.length);
        const lane = forStream(settings, stream, scope);
        return (
          <SettingGroup key={stream} label={port}>
            {streamedAntenna && (
              <SettingRow label="Antenna">
                <Select
                  label={`${port} antenna`}
                  value={lane.antenna ?? caps.antennas[0] ?? ""}
                  options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
                  onChange={(antenna) => patch({ streams: [{ stream, antenna }] })}
                />
              </SettingRow>
            )}
            {streamedGain &&
              caps.gains.map((stage) => (
                <GainControl
                  key={stage.name}
                  stage={stage}
                  port={port}
                  disabled={automaticGain}
                  value={
                    lane.gains?.find((g) => g.stage === stage.name)?.value_db ?? stage.range.min
                  }
                  onCommit={(db) =>
                    patch({ streams: [{ stream, gains: [{ stage: stage.name, value_db: db }] }] })
                  }
                />
              ))}
          </SettingGroup>
        );
      })}

      {caps.bias_tee === true && (
        <SettingRow label="Bias tee" title="Powers an amplifier or active antenna over the coax">
          <Checkbox
            label="Bias tee"
            checked={settings.bias_tee ?? false}
            onChange={(bias_tee) => patch({ bias_tee })}
          />
        </SettingRow>
      )}

      {caps.ppm && (
        <SettingRow label="PPM" title="Frequency correction in parts per million">
          <NumberField
            label="Frequency correction (ppm)"
            value={settings.ppm ?? 0}
            step={1}
            onCommit={(ppm) => patch({ ppm })}
          />
        </SettingRow>
      )}

      {isTunable(tuningRange(caps)) && (
        <SettingRow
          label="Converter (MHz)"
          title="Local oscillator of a converter in front of the radio: positive for a downconverter, negative for an upconverter. Frequencies shown are what the antenna sees"
        >
          <NumberField
            label="Converter offset (MHz)"
            value={(settings.offset_hz ?? 0) / 1e6}
            step={0.001}
            onCommit={(mhz) => patch({ offset_hz: Math.round(mhz * 1e6) })}
          />
        </SettingRow>
      )}

      {hasDcArtifact(caps) && (
        <SettingRow label="DC block" title="Notches the centre bin">
          <Checkbox
            label="Remove the receiver's own DC spike"
            checked={dcBlockOn(caps, settings)}
            onChange={(dc_block) => patch({ dc_block })}
          />
        </SettingRow>
      )}

      {extras.map((setting) => (
        <ExtraControl
          key={setting.name}
          setting={setting}
          raw={settings.extra?.find((e) => e.name === setting.name)?.value}
          onCommit={(value) => patch({ extra: [{ name: setting.name, value }] })}
        />
      ))}
    </Settings>
  );
}

function RateControl({
  caps,
  sampleRate,
  locked,
  onCommit,
}: {
  caps: Capabilities;
  sampleRate: number;
  locked: boolean;
  onCommit: (hz: number) => void;
}) {
  const rateRange = spanOf(caps.sample_rate_ranges);
  if (locked || (caps.sample_rates.length === 1 && rateRange == null)) {
    return (
      <span
        className="font-mono text-xs text-ink"
        title={locked ? "Change the sample rate on the connected Array node" : undefined}
      >
        {formatSampleRate(sampleRate)}
      </span>
    );
  }
  if (caps.sample_rates.length > 0) {
    return (
      <Select
        label="Sample rate"
        value={sampleRate}
        options={withCurrent(
          sampleRate,
          caps.sample_rates.map((rate) => ({ value: rate, label: formatSampleRate(rate) })),
          formatSampleRate,
        )}
        onChange={onCommit}
      />
    );
  }
  return (
    <>
      <NumberField
        label="Sample rate (MS/s)"
        value={sampleRate / 1e6}
        min={rateRange ? rateRange.min / 1e6 : undefined}
        max={rateRange ? rateRange.max / 1e6 : undefined}
        step={rateRange?.step != null ? rateRange.step / 1e6 : 0.001}
        onCommit={(msps) => onCommit(snapToRanges(caps.sample_rate_ranges, Math.round(msps * 1e6)))}
        className="w-24"
      />
      <span className="legend">MS/s</span>
    </>
  );
}

function FilterControl({
  active,
  onCommit,
}: {
  active: DeviceSet;
  onCommit: (bandwidth: DeviceSet["settings"]["bandwidth"]) => void;
}) {
  const caps = active.capabilities;
  const settings = active.settings;
  const auto = filterIsAuto(settings);
  const hz = filterHz(caps, settings);
  const bandwidthRange = spanOf(caps.bandwidth_ranges);
  return (
    <>
      {caps.bandwidth_auto === true && (
        <label
          className="flex items-center gap-1.5"
          title="Let the radio match the filter to the rate"
        >
          <Checkbox
            label="Automatic filter"
            checked={auto}
            onChange={(on) => onCommit(on ? AUTO_FILTER : manualFilter(hz))}
          />
          <span className="legend">Auto</span>
        </label>
      )}
      {caps.bandwidths.length > 0 ? (
        <Select
          label="Analog bandwidth"
          value={hz}
          disabled={auto}
          options={withCurrent(
            hz,
            caps.bandwidths.map((width) => ({ value: width, label: formatHz(width) })),
            formatHz,
          )}
          onChange={(width) => onCommit(manualFilter(width))}
        />
      ) : (
        bandwidthRange != null && (
          <>
            <NumberField
              label="Analog bandwidth (MHz)"
              value={hz / 1e6}
              min={bandwidthRange.min / 1e6}
              max={bandwidthRange.max / 1e6}
              step={0.01}
              disabled={auto}
              onCommit={(mhz) =>
                onCommit(manualFilter(snapToRanges(caps.bandwidth_ranges, Math.round(mhz * 1e6))))
              }
              className="w-24"
            />
            <span className="legend">MHz</span>
          </>
        )
      )}
    </>
  );
}

function AgcControl({
  active,
  onCommit,
}: {
  active: DeviceSet;
  onCommit: (agc: NonNullable<DeviceSet["settings"]["agc"]>) => void;
}) {
  const agc = active.capabilities.agc;
  const state = agcState(active.capabilities, active.settings);
  const modes = agc?.kind === "modes" ? agc.options : [];
  return (
    <>
      <Checkbox
        label="Automatic gain"
        checked={state.on}
        onChange={(on) => onCommit({ ...state, on })}
      />
      {modes.length > 0 && (
        <Select
          label="AGC mode"
          value={state.mode ?? ""}
          disabled={!state.on}
          options={modes.map((mode) => ({ value: mode.value, label: mode.label ?? mode.value }))}
          onChange={(mode) => onCommit({ on: true, mode })}
        />
      )}
    </>
  );
}

function GainControl({
  stage,
  value,
  onCommit,
  port,
  disabled,
}: {
  stage: GainStage;
  value: number;
  onCommit: (db: number) => void;
  port?: string;
  disabled?: boolean;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  const name = gainLabel(stage);
  const unit = gainUnit(stage);
  const label = `${port === undefined ? "" : `${port} `}${name} gain`;
  const title = disabled ? AGC_HINT : unit === "" ? `${name}, firmware step` : `${name} gain in dB`;

  if (isSwitch(stage)) {
    const on = shown > stage.range.min;
    return (
      <SettingRow label={name} title={title}>
        <Checkbox
          label={label}
          checked={on}
          disabled={disabled}
          onChange={(next) => onCommit(next ? stage.range.max : stage.range.min)}
        />
        <span className={READOUT}>
          {on ? `+${stage.range.max.toFixed(0)}` : "0"} <span className="text-ink-faint">dB</span>
        </span>
      </SettingRow>
    );
  }

  const settings = stageSettings(stage);
  return (
    <SettingRow label={name} title={title}>
      {settings.length > 0 ? (
        <Slider
          label={label}
          className="min-w-0 flex-1"
          min={0}
          max={settings.length - 1}
          step={1}
          value={settingIndex(settings, shown)}
          disabled={disabled}
          onChange={(index) => change(settings[index] ?? shown)}
        />
      ) : (
        <Slider
          label={label}
          className="min-w-0 flex-1"
          min={stage.range.min}
          max={stage.range.max}
          step={0.1}
          value={shown}
          disabled={disabled}
          onChange={(db) => change(snapToStage(stage, db))}
        />
      )}
      <span className={READOUT}>
        {formatGain(stage, shown)} <span className="text-ink-faint">{unit}</span>
      </span>
    </SettingRow>
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
  const authoritative =
    setting.kind === "string" && typeof raw === "string"
      ? raw
      : setting.kind === "string"
        ? setting.default
        : "";
  const [draft, setDraft] = useState(authoritative);
  const [dirty, setDirty] = useState(false);
  if (!dirty && draft !== authoritative) {
    setDraft(authoritative);
  }

  const name = setting.label ?? settingLabel(setting.name);
  switch (setting.kind) {
    case "bool":
      return (
        <SettingRow label={name} title={setting.name}>
          <Checkbox
            label={name}
            checked={typeof raw === "boolean" ? raw : setting.default}
            onChange={onCommit}
          />
        </SettingRow>
      );
    case "enum": {
      const options = setting.options.map((option) => ({
        value: option.value,
        label: option.label ?? option.value,
      }));
      const Picker = options.length > SEARCHABLE_FROM ? SearchableSelect : Select;
      return (
        <SettingRow label={name} title={setting.name}>
          <Picker
            label={name}
            value={typeof raw === "string" ? raw : setting.default}
            options={options}
            onChange={onCommit}
          />
        </SettingRow>
      );
    }
    case "range": {
      const value = typeof raw === "number" ? raw : setting.range.min;
      if (fitsSlider(setting.range)) {
        return (
          <RangeSlider
            name={name}
            title={setting.name}
            unit={setting.unit}
            range={setting.range}
            value={value}
            onCommit={onCommit}
          />
        );
      }
      return (
        <SettingRow label={name} title={setting.name}>
          <NumberField
            label={`${name} (${setting.unit})`}
            value={value}
            min={setting.range.min}
            max={setting.range.max}
            step={setting.range.step ?? undefined}
            onCommit={onCommit}
          />
          <span className="legend">{setting.unit}</span>
        </SettingRow>
      );
    }
    case "string":
      return (
        <SettingRow label={name} title={setting.name}>
          <Input
            aria-label={name}
            className={`${FIELD} w-full max-w-64`}
            value={draft}
            onChange={(event) => {
              setDraft(event.currentTarget.value);
              setDirty(true);
            }}
            onBlur={() => {
              onCommit(draft);
              setDirty(false);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
        </SettingRow>
      );
  }
}

function RangeSlider({
  name,
  title,
  unit,
  range,
  value,
  onCommit,
}: {
  name: string;
  title: string;
  unit: string;
  range: Range;
  value: number;
  onCommit: (value: number) => void;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  const digits = range.step != null && range.step < 1 ? 1 : 0;
  return (
    <SettingRow label={name} title={`${title}, ${range.min} to ${range.max}`}>
      <Slider
        label={`${name} (${unit})`}
        className="min-w-0 flex-1"
        min={range.min}
        max={range.max}
        step={range.step ?? 1}
        value={shown}
        onChange={change}
      />
      <span className={READOUT}>
        {shown.toFixed(digits)} <span className="text-ink-faint">{unit}</span>
      </span>
    </SettingRow>
  );
}
