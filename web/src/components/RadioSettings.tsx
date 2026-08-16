import { useEffect, useState } from "react";
import { rxStreamCount, streamLabel } from "../canvas/graph";
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { forStream, useDevicePatch } from "../lib/useDevicePatch";
import { Input } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import {
  isSwitch,
  settingIndex,
  snapToRanges,
  snapToStage,
  spanOf,
  stageSettings,
} from "./capabilities";
import { FIELD } from "./controls";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { LOOP_SETTING } from "./playback";
import { Select } from "./Select";
import { SettingGroup, SettingRow, Settings } from "./Settings";
import { Slider } from "./Slider";
import { withCurrent } from "./selectOptions";
import { settingLabel } from "./settingLabel";
import { useDebouncedCommit } from "./useDebouncedCommit";

const formatMsps = (hz: number): string => `${(hz / 1e6).toFixed(3)} MS/s`;

export function RadioSettings({ active, className }: { active: DeviceSet; className?: string }) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;
  const sampleRate = settings.sample_rate ?? 0;
  const rateRange = spanOf(caps.sample_rate_ranges);
  const bandwidthRange = spanOf(caps.bandwidth_ranges);
  const bandwidth = settings.bandwidth ?? caps.bandwidths[0] ?? 0;
  const extras = (caps.extra ?? []).filter(
    (setting) => active.playback == null || setting.name !== LOOP_SETTING,
  );
  const scope = caps.per_stream;
  const streamedAntenna = scope?.antenna === true && caps.antennas.length > 1;
  const streamedGain = scope?.gain === true && caps.gains.length > 0;
  const streams =
    streamedAntenna || streamedGain
      ? Array.from({ length: rxStreamCount(caps) }, (_, index) => index)
      : [];

  return (
    <Settings className={className}>
      <SettingRow label="Rate">
        {caps.sample_rates.length === 1 && rateRange == null ? (
          <span className="font-mono text-xs text-ink">{formatMsps(sampleRate)}</span>
        ) : caps.sample_rates.length > 0 ? (
          <Select
            label="Sample rate"
            value={sampleRate}
            options={withCurrent(
              sampleRate,
              caps.sample_rates.map((rate) => ({ value: rate, label: formatMsps(rate) })),
              formatMsps,
            )}
            onChange={(sample_rate) => applyPatch(active.id, { sample_rate })}
          />
        ) : (
          <>
            <NumberField
              label="Sample rate (MS/s)"
              value={sampleRate / 1e6}
              min={rateRange ? rateRange.min / 1e6 : undefined}
              max={rateRange ? rateRange.max / 1e6 : undefined}
              step={rateRange?.step != null ? rateRange.step / 1e6 : 0.001}
              onCommit={(msps) =>
                applyPatch(active.id, {
                  sample_rate: snapToRanges(caps.sample_rate_ranges, Math.round(msps * 1e6)),
                })
              }
              className="w-24"
            />
            <span className="legend">MS/s</span>
          </>
        )}
      </SettingRow>

      {caps.bandwidths.length > 0 ? (
        <SettingRow label="Filter">
          <Select
            label="Analog bandwidth"
            value={bandwidth}
            options={withCurrent(
              bandwidth,
              caps.bandwidths.map((hz) => ({ value: hz, label: formatHz(hz) })),
              formatHz,
            )}
            onChange={(hz) => applyPatch(active.id, { bandwidth: hz })}
          />
        </SettingRow>
      ) : (
        bandwidthRange != null && (
          <SettingRow label="Filter">
            <NumberField
              label="Analog bandwidth (MHz)"
              value={bandwidth / 1e6}
              min={bandwidthRange.min / 1e6}
              max={bandwidthRange.max / 1e6}
              step={0.01}
              onCommit={(mhz) =>
                applyPatch(active.id, {
                  bandwidth: snapToRanges(caps.bandwidth_ranges, Math.round(mhz * 1e6)),
                })
              }
              className="w-24"
            />
            <span className="legend">MHz{bandwidthRange.min === 0 ? ", 0 = auto" : ""}</span>
          </SettingRow>
        )
      )}

      {caps.antennas.length > 1 && !streamedAntenna && (
        <SettingRow label="Antenna">
          <Select
            label="Antenna"
            value={settings.antenna ?? caps.antennas[0] ?? ""}
            options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
            onChange={(antenna) => applyPatch(active.id, { antenna })}
          />
        </SettingRow>
      )}

      {!streamedGain &&
        caps.gains.map((stage) => (
          <GainControl
            key={stage.name}
            stage={stage}
            value={settings.gains?.find((g) => g.stage === stage.name)?.value_db ?? stage.range.min}
            onCommit={(db) =>
              applyPatch(active.id, { gains: [{ stage: stage.name, value_db: db }] })
            }
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
                  onChange={(antenna) => applyPatch(active.id, { streams: [{ stream, antenna }] })}
                />
              </SettingRow>
            )}
            {streamedGain &&
              caps.gains.map((stage) => (
                <GainControl
                  key={stage.name}
                  stage={stage}
                  port={port}
                  value={
                    lane.gains?.find((g) => g.stage === stage.name)?.value_db ?? stage.range.min
                  }
                  onCommit={(db) =>
                    applyPatch(active.id, {
                      streams: [{ stream, gains: [{ stage: stage.name, value_db: db }] }],
                    })
                  }
                />
              ))}
          </SettingGroup>
        );
      })}

      {caps.ppm && (
        <SettingRow label="PPM">
          <NumberField
            label="Frequency correction (ppm)"
            value={settings.ppm ?? 0}
            step={1}
            onCommit={(ppm) => applyPatch(active.id, { ppm })}
            className="w-20"
          />
        </SettingRow>
      )}

      {extras.map((setting) => (
        <ExtraControl
          key={setting.name}
          setting={setting}
          raw={settings.extra?.find((e) => e.name === setting.name)?.value}
          onCommit={(value) => applyPatch(active.id, { extra: [{ name: setting.name, value }] })}
        />
      ))}
    </Settings>
  );
}

function GainControl({
  stage,
  value,
  onCommit,
  port,
}: {
  stage: GainStage;
  value: number;
  onCommit: (db: number) => void;
  port?: string;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  const label = `${port === undefined ? "" : `${port} `}${stage.name} gain (dB)`;

  if (isSwitch(stage)) {
    return (
      <SettingRow label={settingLabel(stage.name)} title={stage.name}>
        <Checkbox
          label={label}
          checked={shown > stage.range.min}
          onChange={(on) => onCommit(on ? stage.range.max : stage.range.min)}
        />
        <span className="w-14 shrink-0 text-right font-mono text-xs text-ink">
          {shown > stage.range.min ? `+${stage.range.max.toFixed(0)}` : "0"}{" "}
          <span className="text-ink-faint">dB</span>
        </span>
      </SettingRow>
    );
  }

  const settings = stage.values?.length ? stageSettings(stage) : [];
  return (
    <SettingRow label={settingLabel(stage.name)} title={stage.name}>
      {settings.length > 0 ? (
        <Slider
          label={label}
          className="min-w-0 flex-1"
          min={0}
          max={settings.length - 1}
          step={1}
          value={settingIndex(settings, shown)}
          onChange={(index) => change(settings[index] ?? shown)}
        />
      ) : (
        <Slider
          label={label}
          className="min-w-0 flex-1"
          min={stage.range.min}
          max={stage.range.max}
          step={stage.range.step ?? 0.1}
          value={shown}
          onChange={(db) => change(snapToStage(stage, db))}
        />
      )}
      <span className="w-14 shrink-0 text-right font-mono text-xs text-ink">
        {shown.toFixed(1)} <span className="text-ink-faint">dB</span>
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
  useEffect(() => {
    if (!dirty) setDraft(authoritative);
  }, [authoritative, dirty]);

  const name = settingLabel(setting.name);
  switch (setting.kind) {
    case "bool":
      return (
        <SettingRow label={name} title={setting.name}>
          <Checkbox
            label={setting.name}
            checked={typeof raw === "boolean" ? raw : setting.default}
            onChange={onCommit}
          />
        </SettingRow>
      );
    case "enum":
      return (
        <SettingRow label={name} title={setting.name}>
          <Select
            label={setting.name}
            value={typeof raw === "string" ? raw : setting.default}
            options={setting.options.map((option) => ({
              value: option.value,
              label: option.label ?? option.value,
            }))}
            onChange={onCommit}
          />
        </SettingRow>
      );
    case "range":
      return (
        <SettingRow label={name} title={setting.name}>
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
        </SettingRow>
      );
    case "string":
      return (
        <SettingRow label={name} title={setting.name}>
          <Input
            aria-label={setting.name}
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
