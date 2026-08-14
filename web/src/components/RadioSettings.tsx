// Capability-driven receiver controls (): everything renders from `Capabilities` +
// `DeviceSettings` alone, so a new device setting needs zero frontend work. One row per setting,
// everything the dial is not — the same rows serve the device node's face (CANVAS §1) and the
// M6 radio popover, because a receiver has one set of controls, not one per surface.
import { useEffect, useState } from "react";
import { rxStreamCount, streamLabel } from "../canvas/graph";
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { forStream, useDevicePatch } from "../lib/useDevicePatch";
import { Checkbox } from "./Checkbox";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { LOOP_SETTING } from "./playback";
import { Select, withCurrent } from "./Select";
import { Slider } from "./Slider";
import { settingLabel } from "./settingLabel";
import { useDebouncedCommit } from "./useDebouncedCommit";

// The label track is measured off the block's own width, not the viewport's: these rows render
// inside a node the operator can drag down to 260 px — where a fixed column would push the
// control off the edge — and out to the width of the desk, where a driver's `digital_agc` should
// read as a name rather than as `DIGITAL_A…`.
const ROW =
  "grid grid-cols-[minmax(0,5.5rem)_minmax(0,1fr)] @xs:grid-cols-[minmax(0,8rem)_minmax(0,1fr)] @md:grid-cols-[minmax(0,11rem)_minmax(0,1fr)] items-center gap-3";

// A dropped-down list is read top to bottom, so past a point extra width only stretches the
// trigger away from its label. Sliders take the rest of the row, where width *is* resolution.
const PICKER = "w-full max-w-64";

const formatMsps = (hz: number): string => `${(hz / 1e6).toFixed(3)} MS/s`;

/** A setting named by its driver: shown as words, hovered as the key itself. */
function SettingName({ name }: { name: string }) {
  return (
    <span className="legend wrap-anywhere" title={name}>
      {settingLabel(name)}
    </span>
  );
}

export function RadioSettings({ active }: { active: DeviceSet }) {
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
    <div className="@container flex flex-col gap-2">
      <div className={ROW}>
        <span className="legend">Rate</span>
        {caps.sample_rates.length === 1 && rateRange == null ? (
          // One rate and nothing to pick between it and: a recording plays at the rate it was
          // captured at, and a single-rate receiver says the same thing. A dropdown of one is
          // a control that cannot act, so this is a readout.
          <span className="font-mono text-xs text-ink">{formatMsps(sampleRate)}</span>
        ) : caps.sample_rates.length > 0 ? (
          <Select
            label="Sample rate"
            className={PICKER}
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
            className={PICKER}
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

      {caps.antennas.length > 1 && !streamedAntenna && (
        <div className={ROW}>
          <span className="legend">Antenna</span>
          <Select
            label="Antenna"
            className={PICKER}
            value={settings.antenna ?? caps.antennas[0] ?? ""}
            options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
            onChange={(antenna) => applyPatch(active.id, { antenna })}
          />
        </div>
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
          <div key={stream} className="flex flex-col gap-2 border-t border-line pt-2">
            <span className="legend">{port}</span>
            {streamedAntenna && (
              <div className={ROW}>
                <span className="legend">Antenna</span>
                <Select
                  label={`${port} antenna`}
                  className={PICKER}
                  value={lane.antenna ?? caps.antennas[0] ?? ""}
                  options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
                  onChange={(antenna) => applyPatch(active.id, { streams: [{ stream, antenna }] })}
                />
              </div>
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
          </div>
        );
      })}

      {/* Only where the radio has a correction to make. HackRF has no register for one and
          SpyServer's protocol no field, so their backends refuse it; a recording and the signal
          generator swallow it and do nothing. Drawn everywhere, the knob looked identical
          whether it worked, errored, or lied. */}
      {caps.ppm && (
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
      )}

      {extras.map((setting) => (
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
    <div className={ROW}>
      <SettingName name={stage.name} />
      <span className="flex items-center gap-2">
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

  switch (setting.kind) {
    case "bool":
      return (
        <div className={ROW}>
          <SettingName name={setting.name} />
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
          <SettingName name={setting.name} />
          <Select
            label={setting.name}
            className={PICKER}
            value={typeof raw === "string" ? raw : setting.default}
            options={setting.options.map((option) => ({
              value: option.value,
              label: option.label ?? option.value,
            }))}
            onChange={onCommit}
          />
        </div>
      );
    case "range":
      return (
        <div className={ROW}>
          <SettingName name={setting.name} />
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
    case "string":
      return (
        <div className={ROW}>
          <SettingName name={setting.name} />
          <input
            aria-label={setting.name}
            className={`${PICKER} min-w-0 rounded border border-line bg-surface px-2 py-1 font-mono text-xs text-ink`}
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
        </div>
      );
  }
}
