// Capability-driven receiver controls (PLAN §6): everything renders from `Capabilities` +
// `DeviceSettings` alone, so a new device setting needs zero frontend work. One row per setting,
// everything the dial is not — the same rows serve the device node's face (CANVAS §1) and the
// M6 radio popover, because a receiver has one set of controls, not one per surface.
import { rxStreamCount, streamLabel } from "../canvas/graph";
import type { DeviceSet, ExtraSetting, GainStage } from "../lib/types";
import { forStream, useDevicePatch } from "../lib/useDevicePatch";
import { Checkbox } from "./Checkbox";
import { formatHz } from "./format";
import { NumberField } from "./NumberField";
import { Select, withCurrent } from "./Select";
import { Slider } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

// The label track gives its width back when there is none to spare: these rows now also render
// inside a node the operator can drag down to 220 px, where a fixed column would push the
// control off the edge.
const ROW = "grid grid-cols-[minmax(0,4.5rem)_1fr] items-center gap-3";

const formatMsps = (hz: number): string => `${(hz / 1e6).toFixed(3)} MS/s`;

export function RadioSettings({ active }: { active: DeviceSet }) {
  const { applyPatch } = useDevicePatch();
  const caps = active.capabilities;
  const settings = active.settings;
  const sampleRate = settings.sample_rate ?? 0;
  const rateRange = caps.sample_rate_range;
  const bandwidth = settings.bandwidth ?? caps.bandwidths[0] ?? 0;
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

      {caps.antennas.length > 1 && !streamedAntenna && (
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
                  className="w-full"
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
      <span className="legend truncate" title={stage.name}>
        {stage.name}
      </span>
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
  switch (setting.kind) {
    case "bool":
      return (
        <div className={ROW}>
          <span className="legend truncate" title={setting.name}>
            {setting.name}
          </span>
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
          <span className="legend truncate" title={setting.name}>
            {setting.name}
          </span>
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
          <span className="legend truncate" title={setting.name}>
            {setting.name}
          </span>
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
