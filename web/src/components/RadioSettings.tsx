import { Minus, Plus } from "lucide-react";
import { type ReactNode, useState } from "react";
import { rxStreamCount, streamLabel } from "../canvas/graph";
import { agcDelta, agcGainDb, agcModeDelta, laneAgc, radioAgc } from "../canvas/nodes/deviceNode";
import { PortAnchor } from "../canvas/nodes/NodeShell";
import { useLevelStore } from "../lib/levels";
import type { Capabilities, DeviceSet, ExtraSetting, GainStage, Range } from "../lib/types";
import { forStream, useDevicePatch } from "../lib/useDevicePatch";
import { AgcAuto, AutoToggle, agcTip } from "./AgcAuto";
import { Button, Input } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import {
  AUTO_FILTER,
  agcDrives,
  agcOffered,
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
import { FIELD, ICON_BTN_SM, LABEL } from "./controls";
import { meterTone } from "./dbfs";
import { isTunable, tuningRange } from "./dial";
import { formatHz, formatSampleRate } from "./format";
import { GainMeter } from "./GainMeter";
import { Icon } from "./Icon";
import {
  allLanesGain,
  laneGain,
  laneGains,
  laneLayout,
  meterStage,
  rxStages,
  spreadOf,
  steppedLanes,
  txStages,
} from "./laneRows";
import { NumberField } from "./NumberField";
import { LOOP_SETTING } from "./playback";
import { SearchableSelect } from "./SearchableSelect";
import { Select } from "./Select";
import { ReadoutChip, SettingChip, ToggleChip } from "./SettingChip";
import { Slider } from "./Slider";
import { withCurrent } from "./selectOptions";
import { settingLabel } from "./settingLabel";
import { Unit } from "./Unit";
import { useDebouncedCommit } from "./useDebouncedCommit";

const SEARCHABLE_FROM = 12;

const WIDE = "min-w-0 flex-1";

const ROW = "grid h-7 grid-cols-[3.5rem_minmax(0,1fr)_3.75rem_2.75rem_0] items-center gap-x-2";

const READOUT = "text-right font-mono text-xs tabular-nums whitespace-nowrap text-ink";

type Patch = (delta: Parameters<ReturnType<typeof useDevicePatch>["applyPatch"]>[1]) => void;

export function RadioSettings({
  active,
  className,
  advised = new Set(),
  lead,
  laneLeads,
  ports,
}: {
  active: DeviceSet;
  className?: string;
  advised?: ReadonlySet<number>;
  lead?: ReactNode;
  laneLeads?: readonly ReactNode[];
  ports?: readonly string[];
}) {
  const { applyPatch } = useDevicePatch();
  const patch: Patch = (delta) => applyPatch(active.id, delta);
  return (
    <div className={`flex flex-col gap-2 ${className ?? ""}`}>
      {lead}
      <SettingChips active={active} patch={patch} />
      <GainLanes
        active={active}
        advised={advised}
        laneLeads={laneLeads}
        ports={ports}
        patch={patch}
      />
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <span className={LABEL}>{label}</span>
      <span className="flex min-w-0 flex-wrap items-center gap-2">{children}</span>
    </div>
  );
}

function SettingChips({ active, patch }: { active: DeviceSet; patch: Patch }) {
  const caps = active.capabilities;
  const settings = active.settings;
  const extras = (caps.extra ?? []).filter(
    (setting) => active.playback == null || setting.name !== LOOP_SETTING,
  );
  const streamedAntenna = caps.per_stream?.antenna === true && caps.antennas.length > 1;
  const agcOnGain = agcOffered(caps) && rxStages(caps).length > 0;
  const agcModes = caps.agc?.kind === "modes";
  const lanes = rxStreamCount(caps);
  return (
    <div className="flex flex-wrap gap-x-1 gap-y-[3px]">
      <RateChip caps={caps} sampleRate={settings.sample_rate ?? 0} patch={patch} />

      {hasFilter(caps) && (
        <SettingChip
          label={caps.bandwidth_auto === true || caps.bandwidths.length > 0 ? "BW" : "Filter"}
          value={filterIsAuto(settings) ? "auto" : formatHz(filterHz(caps, settings))}
          quiet={filterIsAuto(settings)}
          title="Analog bandwidth before the ADC"
        >
          {() => (
            <Field label="Analog bandwidth">
              <FilterControl active={active} onCommit={(bandwidth) => patch({ bandwidth })} />
            </Field>
          )}
        </SettingChip>
      )}

      {caps.antennas.length > 1 && !streamedAntenna && (
        <SettingChip
          label="Antenna"
          value={settings.antenna ?? caps.antennas[0] ?? ""}
          title="Antenna port in use"
        >
          {() => (
            <Field label="Antenna">
              <Select
                className={WIDE}
                label="Antenna"
                value={settings.antenna ?? caps.antennas[0] ?? ""}
                options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
                onChange={(antenna) => patch({ antenna })}
              />
            </Field>
          )}
        </SettingChip>
      )}

      {streamedAntenna &&
        Array.from({ length: lanes }, (_, stream) => {
          const port = streamLabel("iq", stream, lanes);
          const lane = forStream(settings, stream, caps.per_stream);
          return (
            <SettingChip
              key={stream}
              label={`Ant ${port}`}
              value={lane.antenna ?? caps.antennas[0] ?? ""}
              title={`Antenna port feeding ${port}`}
            >
              {() => (
                <Field label={`${port} antenna`}>
                  <Select
                    className={WIDE}
                    label={`${port} antenna`}
                    value={lane.antenna ?? caps.antennas[0] ?? ""}
                    options={caps.antennas.map((antenna) => ({ value: antenna, label: antenna }))}
                    onChange={(antenna) => patch({ streams: [{ stream, antenna }] })}
                  />
                </Field>
              )}
            </SettingChip>
          );
        })}

      {agcOffered(caps) && !agcOnGain && !agcModes && (
        <ToggleChip
          label="AGC"
          on={radioAgc(active).on}
          title="The radio sets its own gain"
          onChange={(on) => patch({ agc: { ...radioAgc(active), on } })}
        />
      )}

      {agcOffered(caps) && agcModes && (
        <AgcChip active={active} toggle={!agcOnGain} patch={patch} />
      )}

      {caps.ppm && (
        <SettingChip
          label="PPM"
          value={String(settings.ppm ?? 0)}
          quiet={(settings.ppm ?? 0) === 0}
          title="Frequency correction in parts per million, kept for this radio"
          width="w-48"
        >
          {() => (
            <Field label="Frequency correction">
              <NumberField
                className={WIDE}
                label="Frequency correction"
                unit="ppm"
                value={settings.ppm ?? 0}
                step={1}
                onCommit={(ppm) => patch({ ppm })}
              />
            </Field>
          )}
        </SettingChip>
      )}

      {isTunable(tuningRange(caps)) && (
        <SettingChip
          label="Conv"
          value={settings.offset_hz ? `${settings.offset_hz / 1e6} MHz` : "none"}
          quiet={!settings.offset_hz}
          title="Local oscillator of a converter in front of the radio: positive for a downconverter, negative for an upconverter. Frequencies shown are what the antenna sees. Kept for this radio"
          width="w-56"
        >
          {() => (
            <Field label="Converter offset">
              <NumberField
                className={WIDE}
                label="Converter offset"
                unit="MHz"
                value={(settings.offset_hz ?? 0) / 1e6}
                step={0.001}
                onCommit={(mhz) => patch({ offset_hz: Math.round(mhz * 1e6) })}
              />
            </Field>
          )}
        </SettingChip>
      )}

      {hasDcArtifact(caps) && (
        <ToggleChip
          label="DC block"
          on={dcBlockOn(caps, settings)}
          title="Remove the receiver's own DC spike by notching the centre bin"
          onChange={(dc_block) => patch({ dc_block })}
        />
      )}

      {caps.bias_tee === true && (
        <ToggleChip
          label="Bias tee"
          on={settings.bias_tee ?? false}
          title="Powers an amplifier or active antenna over the coax"
          onChange={(bias_tee) => patch({ bias_tee })}
        />
      )}

      {extras.map((setting) => (
        <ExtraChip
          key={setting.name}
          setting={setting}
          raw={settings.extra?.find((e) => e.name === setting.name)?.value}
          onCommit={(value) => patch({ extra: [{ name: setting.name, value }] })}
        />
      ))}
    </div>
  );
}

function RateChip({
  caps,
  sampleRate,
  patch,
}: {
  caps: Capabilities;
  sampleRate: number;
  patch: Patch;
}) {
  const rateRange = spanOf(caps.sample_rate_ranges);
  const shown = formatSampleRate(sampleRate);
  if (caps.sample_rates.length === 1 && rateRange == null) {
    return <ReadoutChip label="Rate" value={shown} title="Sample rate, fixed by the radio" />;
  }
  return (
    <SettingChip label="Rate" value={shown} title="Sample rate">
      {() => (
        <Field label="Sample rate">
          <RateControl
            caps={caps}
            sampleRate={sampleRate}
            onCommit={(sample_rate) => patch({ sample_rate })}
          />
        </Field>
      )}
    </SettingChip>
  );
}

function AgcChip({ active, toggle, patch }: { active: DeviceSet; toggle: boolean; patch: Patch }) {
  const agc = active.capabilities.agc;
  const state = radioAgc(active);
  const modes = agc?.kind === "modes" ? agc.options : [];
  const current = modes.find((mode) => mode.value === state.mode);
  return (
    <SettingChip
      label="AGC"
      value={state.on || !toggle ? (current?.label ?? state.mode ?? "") : "off"}
      quiet={toggle && !state.on}
      title="How the radio sets its own gain"
    >
      {() => (
        <Field label="Gain control">
          {toggle && (
            <Checkbox
              label="Automatic gain"
              checked={state.on}
              onChange={(on) => patch({ agc: { ...state, on } })}
            />
          )}
          <Select
            className={WIDE}
            label="AGC mode"
            value={state.mode ?? ""}
            disabled={toggle && !state.on}
            options={modes.map((mode) => ({ value: mode.value, label: mode.label ?? mode.value }))}
            onChange={(mode) => patch(agcModeDelta(active, mode))}
          />
        </Field>
      )}
    </SettingChip>
  );
}

function RateControl({
  caps,
  sampleRate,
  onCommit,
}: {
  caps: Capabilities;
  sampleRate: number;
  onCommit: (hz: number) => void;
}) {
  const rateRange = spanOf(caps.sample_rate_ranges);
  if (caps.sample_rates.length > 0) {
    return (
      <Select
        className={WIDE}
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
    <NumberField
      label="Sample rate"
      unit="MS/s"
      value={sampleRate / 1e6}
      min={rateRange ? rateRange.min / 1e6 : undefined}
      max={rateRange ? rateRange.max / 1e6 : undefined}
      step={rateRange?.step != null ? rateRange.step / 1e6 : 0.001}
      onCommit={(msps) => onCommit(snapToRanges(caps.sample_rate_ranges, Math.round(msps * 1e6)))}
      className={WIDE}
    />
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
          className={WIDE}
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
          <NumberField
            label="Analog bandwidth"
            unit="MHz"
            value={hz / 1e6}
            min={bandwidthRange.min / 1e6}
            max={bandwidthRange.max / 1e6}
            step={0.01}
            disabled={auto}
            onCommit={(mhz) =>
              onCommit(manualFilter(snapToRanges(caps.bandwidth_ranges, Math.round(mhz * 1e6))))
            }
            className={WIDE}
          />
        )
      )}
    </>
  );
}

function ExtraChip({
  setting,
  raw,
  onCommit,
}: {
  setting: ExtraSetting;
  raw: unknown;
  onCommit: (value: boolean | string | number) => void;
}) {
  const name = setting.label ?? settingLabel(setting.name);
  switch (setting.kind) {
    case "bool":
      return (
        <ToggleChip
          label={name}
          on={typeof raw === "boolean" ? raw : setting.default}
          title={name}
          onChange={onCommit}
        />
      );
    case "enum": {
      const options = setting.options.map((option) => ({
        value: option.value,
        label: option.label ?? option.value,
      }));
      const value = typeof raw === "string" ? raw : setting.default;
      const Picker = options.length > SEARCHABLE_FROM ? SearchableSelect : Select;
      return (
        <SettingChip
          label={name}
          value={options.find((option) => option.value === value)?.label ?? value}
          title={name}
        >
          {() => (
            <Field label={name}>
              <Picker
                className={WIDE}
                label={name}
                value={value}
                options={options}
                onChange={onCommit}
              />
            </Field>
          )}
        </SettingChip>
      );
    }
    case "range": {
      const value = typeof raw === "number" ? raw : setting.range.min;
      return (
        <SettingChip
          label={name}
          value={String(value)}
          unit={setting.unit === "" ? undefined : setting.unit}
          title={`${name}: ${setting.range.min} to ${setting.range.max}`}
        >
          {() =>
            fitsSlider(setting.range) ? (
              <RangeSlider
                name={name}
                unit={setting.unit}
                range={setting.range}
                value={value}
                onCommit={onCommit}
              />
            ) : (
              <Field label={name}>
                <NumberField
                  className={WIDE}
                  label={name}
                  unit={setting.unit === "" ? undefined : setting.unit}
                  value={value}
                  min={setting.range.min}
                  max={setting.range.max}
                  step={setting.range.step ?? undefined}
                  onCommit={onCommit}
                />
              </Field>
            )
          }
        </SettingChip>
      );
    }
    case "string": {
      const value = typeof raw === "string" ? raw : setting.default;
      return (
        <SettingChip
          label={name}
          value={value === "" ? "none" : value}
          quiet={value === ""}
          title={name}
        >
          {(close) => (
            <Field label={name}>
              <TextEntry label={name} value={value} onCommit={onCommit} onDone={close} />
            </Field>
          )}
        </SettingChip>
      );
    }
  }
}

function TextEntry({
  label,
  value,
  onCommit,
  onDone,
}: {
  label: string;
  value: string;
  onCommit: (value: string) => void;
  onDone: () => void;
}) {
  const [draft, setDraft] = useState(value);
  return (
    <Input
      autoFocus
      aria-label={label}
      className={`${FIELD} ${WIDE}`}
      value={draft}
      onChange={(event) => setDraft(event.currentTarget.value)}
      onBlur={() => onCommit(draft)}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          onCommit(draft);
          onDone();
        }
      }}
    />
  );
}

function RangeSlider({
  name,
  unit,
  range,
  value,
  onCommit,
}: {
  name: string;
  unit: string;
  range: Range;
  value: number;
  onCommit: (value: number) => void;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  const digits = range.step != null && range.step < 1 ? 1 : 0;
  return (
    <Field label={name}>
      <Slider
        label={`${name} (${unit})`}
        className="min-w-0 flex-1"
        min={range.min}
        max={range.max}
        step={range.step ?? 1}
        value={shown}
        onChange={change}
      />
      <span className="w-14 shrink-0 text-right font-mono text-xs tabular-nums text-ink">
        {shown.toFixed(digits)} <Unit symbol={unit} className="text-ink-faint" />
      </span>
    </Field>
  );
}

function GainLanes({
  active,
  advised,
  laneLeads,
  ports,
  patch,
}: {
  active: DeviceSet;
  advised: ReadonlySet<number>;
  laneLeads?: readonly ReactNode[];
  ports?: readonly string[];
  patch: Patch;
}) {
  const caps = active.capabilities;
  const layout = laneLayout(caps);
  const stages = rxStages(caps);
  const metered = meterStage(caps);
  const peaks = useLevelStore((state) => state.lanesByDeviceSet[active.id]);
  const clipping = new Set(active.clipping ?? []);
  const transmit = txStages(caps);
  if (stages.length === 0 && transmit.length === 0 && !layout.stepper && laneLeads === undefined) {
    return null;
  }
  const lanes = Array.from({ length: layout.lanes }, (_, stream) => stream);
  return (
    <div className="flex flex-col gap-px">
      {layout.master && metered !== undefined && (
        <MasterRow
          active={active}
          stage={metered}
          layout={layout}
          peakDb={Math.max(...lanes.map((stream) => peaks?.[stream] ?? Number.NEGATIVE_INFINITY))}
          clipping={lanes.some((stream) => clipping.has(stream))}
          port={layout.lanes === 1 ? ports?.[0] : undefined}
          patch={patch}
        />
      )}
      {(layout.lanes > 1 || !layout.master || laneLeads !== undefined) &&
        lanes.map((stream) => (
          <LaneRows
            key={stream}
            active={active}
            stream={stream}
            stages={stages}
            metered={metered}
            peakDb={peaks?.[stream]}
            clipping={clipping.has(stream)}
            advised={advised.has(stream)}
            lead={laneLeads?.[stream]}
            port={ports?.[stream]}
            laneName={layout.perLane ? streamLabel("iq", stream, layout.lanes) : undefined}
            patch={patch}
          />
        ))}
      {transmit.map((stage) => (
        <StageRow
          key={stage.name}
          label="TX"
          stage={stage}
          value={
            active.settings.gains?.find((gain) => gain.stage === stage.name)?.value_db ??
            stage.range.min
          }
          onCommit={(value_db) => patch({ gains: [{ stage: stage.name, value_db }] })}
        />
      ))}
    </div>
  );
}

function LaneRows({
  active,
  stream,
  stages,
  metered,
  peakDb,
  clipping,
  advised,
  lead,
  port,
  laneName,
  patch,
}: {
  active: DeviceSet;
  stream: number;
  stages: GainStage[];
  metered: GainStage | undefined;
  peakDb: number | undefined;
  clipping: boolean;
  advised: boolean;
  lead: ReactNode;
  port: string | undefined;
  laneName: string | undefined;
  patch: Patch;
}) {
  const caps = active.capabilities;
  const agc = laneAgc(active, stream);
  const agcHere = agcOffered(caps) && (caps.per_stream?.agc === true || stream === 0);
  const measured = agcGainDb(active, stream);
  return (
    <>
      {lead}
      {stages.map((stage, index) => {
        const driven = agcDrives(stage, agc);
        const value = laneGains(active, stage)[stream] ?? stage.range.min;
        return (
          <StageRow
            key={stage.name}
            label={
              index === 0 && laneName !== undefined
                ? lead === undefined
                  ? laneName
                  : ""
                : gainLabel(stage)
            }
            lane={laneName}
            stage={stage}
            value={driven ? (measured ?? value) : value}
            auto={driven}
            peakDb={stage === metered ? (peakDb ?? null) : undefined}
            tone={meterTone(peakDb, clipping)}
            port={index === 0 ? port : undefined}
            trailing={
              stage === metered && agcHere ? (
                <AgcAuto set={active} stream={stream} port={laneName} advised={advised} />
              ) : undefined
            }
            onCommit={(value_db) => {
              if (driven) {
                patch(agcDelta(caps, stream, { ...agc, on: false }));
              }
              patch(laneGain(caps, stream, stage, value_db));
            }}
          />
        );
      })}
    </>
  );
}

function MasterRow({
  active,
  stage,
  layout,
  peakDb,
  clipping,
  port,
  patch,
}: {
  active: DeviceSet;
  stage: GainStage;
  layout: ReturnType<typeof laneLayout>;
  peakDb: number;
  clipping: boolean;
  port: string | undefined;
  patch: Patch;
}) {
  const caps = active.capabilities;
  const lanes = Array.from({ length: layout.lanes }, (_, stream) => stream);
  const autos = lanes.map((stream) => laneAgc(active, stream));
  const driven = autos.map((agc) => agcDrives(stage, agc));
  const gains = laneGains(active, stage).map((value, stream) =>
    driven[stream] === true ? (agcGainDb(active, stream) ?? value) : value,
  );
  const spread = spreadOf(gains);
  const pressed = driven.every(Boolean) ? true : driven.some(Boolean) ? "mixed" : false;
  const setAll = (on: boolean): void => {
    const changes = lanes.filter((stream) => autos[stream]?.on !== on);
    for (const stream of changes) {
      patch(agcDelta(caps, stream, { ...(autos[stream] ?? { on }), on }));
    }
  };
  return (
    <StageRow
      label={
        layout.stepper ? (
          <LaneStepper caps={caps} patch={patch} />
        ) : (
          <span className="legend">All</span>
        )
      }
      stage={stage}
      value={spread.mean}
      readout={spread.uniform ? undefined : "mixed"}
      auto={pressed === true}
      peakDb={Number.isFinite(peakDb) ? peakDb : null}
      tone={meterTone(Number.isFinite(peakDb) ? peakDb : undefined, clipping)}
      port={port}
      trailing={
        agcOffered(caps) ? (
          <AutoToggle
            label="Automatic gain on every lane"
            pressed={pressed}
            title={agcTip(active, 0, false)}
            onChange={setAll}
          />
        ) : undefined
      }
      onCommit={(value_db) => {
        setAll(false);
        patch(allLanesGain(caps, stage, value_db));
      }}
    />
  );
}

function LaneStepper({ caps, patch }: { caps: Capabilities; patch: Patch }) {
  const choices = caps.rx_stream_choices ?? [];
  const current = caps.rx_streams ?? 1;
  const step = (direction: number): void =>
    patch({ rx_streams: steppedLanes(choices, current, direction) });
  const first = Math.min(...choices);
  const last = Math.max(...choices);
  return (
    <span
      role="group"
      aria-label="Lane count"
      title="Receive lanes streamed. Fewer lanes get more rate each"
      className="inline-flex h-5.5 items-center rounded-[3px] border border-line bg-well"
    >
      <Button
        type="button"
        className={ICON_BTN_SM}
        aria-label="Fewer lanes"
        disabled={current <= first}
        onClick={() => step(-1)}
      >
        <Icon glyph={Minus} size={12} />
      </Button>
      <b className="min-w-4 text-center font-mono text-xs font-medium tabular-nums">{current}</b>
      <Button
        type="button"
        className={ICON_BTN_SM}
        aria-label="More lanes"
        disabled={current >= last}
        onClick={() => step(1)}
      >
        <Icon glyph={Plus} size={12} />
      </Button>
    </span>
  );
}

function StageRow({
  label,
  lane,
  stage,
  value,
  readout,
  auto = false,
  peakDb,
  tone = "ok",
  port,
  trailing,
  onCommit,
}: {
  label: ReactNode;
  lane?: string;
  stage: GainStage;
  value: number;
  readout?: string;
  auto?: boolean;
  peakDb?: number | null;
  tone?: ReturnType<typeof meterTone>;
  port?: string;
  trailing?: ReactNode;
  onCommit: (db: number) => void;
}) {
  const { pending, change } = useDebouncedCommit(onCommit);
  const shown = pending ?? value;
  const name = gainLabel(stage);
  const unit = gainUnit(stage);
  const control = `${lane === undefined ? "" : `${lane} `}${name} gain`;
  const settings = stageSettings(stage);
  const lead =
    typeof label === "string" ? (
      <span
        className={`truncate ${lane !== undefined && label === lane ? "font-mono text-[11px] text-port-iq" : "legend"}`}
        title={unit === "" ? "Firmware step, not dB" : undefined}
      >
        {label}
      </span>
    ) : (
      label
    );

  if (isSwitch(stage)) {
    const on = shown > stage.range.min;
    return (
      <div className={ROW}>
        {lead}
        <span className="flex items-center">
          <Checkbox
            label={control}
            checked={on}
            onChange={(next) => onCommit(next ? stage.range.max : stage.range.min)}
          />
        </span>
        <span className={READOUT}>
          {on ? `+${stage.range.max.toFixed(0)}` : "0"}{" "}
          <Unit symbol="dB" className="text-ink-faint" />
        </span>
        <span />
        {port !== undefined && <PortAnchor port={port} />}
      </div>
    );
  }

  const slider =
    settings.length > 0 ? (
      <GainMeter
        label={control}
        className={WIDE}
        min={0}
        max={settings.length - 1}
        step={1}
        value={settingIndex(settings, shown)}
        auto={auto}
        peakDb={peakDb}
        tone={tone}
        onChange={(index) => change(settings[index] ?? shown)}
      />
    ) : (
      <GainMeter
        label={control}
        className={WIDE}
        min={stage.range.min}
        max={stage.range.max}
        step={0.1}
        value={shown}
        auto={auto}
        peakDb={peakDb}
        tone={tone}
        onChange={(db) => change(snapToStage(stage, db))}
      />
    );
  return (
    <div className={ROW}>
      {lead}
      {slider}
      <span className={`${READOUT} ${readout === undefined ? "" : "text-ink-faint"}`}>
        {readout ?? (
          <>
            {formatGain(stage, shown)} <Unit symbol={unit} className="text-ink-faint" />
          </>
        )}
      </span>
      {trailing ?? <span />}
      {port !== undefined && <PortAnchor port={port} />}
    </div>
  );
}
