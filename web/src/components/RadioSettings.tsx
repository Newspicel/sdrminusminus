import { useEffect, useState } from "react";
import { rxStreamCount, streamLabel } from "../canvas/graph";
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { forStream, useDevicePatch } from "../lib/useDevicePatch";
import { Input } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { FIELD } from "./controls";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { LOOP_SETTING } from "./playback";
import { Select, withCurrent } from "./Select";
import { SettingGroup, SettingRow, Settings } from "./Settings";
import { Slider } from "./Slider";
import { settingLabel } from "./settingLabel";
import { useDebouncedCommit } from "./useDebouncedCommit";

const formatMsps = (hz: number): string => `${(hz / 1e6).toFixed(3)} MS/s`;

export function RadioSettings({ active, className }: { active: DeviceSet; className?: string }) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;
  const sampleRate = settings.sample_rate ?? 0;
  const rateRange = caps.sample_rate_range;
  const bandwidth = settings.bandwidth ?? caps.bandwidths[0] ?? 0;
  // A replaying set draws `loop` as a transport button instead; two controls for one setting
  // would be two places to read the same answer, and they would disagree mid-flight.
  const extras = (caps.extra ?? []).filter(
    (setting) => active.playback == null || setting.name !== LOOP_SETTING,
  );
  // A setting the radio scopes per-stream moves out of the shared rows and into one block per
  // lane, named after the IQ port it feeds — one control per thing the radio can actually hold,
  // never a shared knob quietly writing four lanes at once (Capabilities::per_stream).
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
          // One rate and nothing to pick between it and: a recording plays at the rate it was
          // captured at, and a single-rate receiver says the same thing. A dropdown of one is a
          // control that cannot act, so this is a readout.
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
              onCommit={(msps) => applyPatch(active.id, { sample_rate: Math.round(msps * 1e6) })}
              className="w-24"
            />
            <span className="legend">MS/s</span>
          </>
        )}
      </SettingRow>

      {caps.bandwidths.length > 0 && (
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
        // The lane's resolved view: its override where one exists, the radio-wide value
        // otherwise — the same fallback the engine applies, so the control shows what the lane
        // is actually running at.
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

      {/* Only where the radio has a correction to make. HackRF has no register for one and
          SpyServer's protocol no field, so their backends refuse it; a recording and the signal
          generator swallow it and do nothing. Drawn everywhere, the knob looked identical
          whether it worked, errored, or lied. */}
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
  /** The IQ port whose lane this stage belongs to. Only the accessible name carries it — the
   * row already sits under its stream's header, but per-stream radios repeat every stage name
   * and a screen reader needs the sliders told apart. */
  port?: string;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  return (
    <SettingRow label={settingLabel(stage.name)} title={stage.name}>
      <Slider
        label={`${port === undefined ? "" : `${port} `}${stage.name} gain (dB)`}
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
