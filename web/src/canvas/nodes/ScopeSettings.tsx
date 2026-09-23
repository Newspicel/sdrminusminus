import { Settings2 } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "../../components/BaseControls";
import { plotButton } from "../../components/controls";
import { DB_LIMIT, DB_STEP, withCeiling, withFloor } from "../../components/dbRange";
import { Icon } from "../../components/Icon";
import { Popover } from "../../components/Popover";
import { Segmented } from "../../components/Segmented";
import { SettingsPanel, SettingsSection, ToggleChip } from "../../components/SettingsPanel";
import { Slider } from "../../components/Slider";
import { Switch } from "../../components/Switch";
import { type DbWindow, TRACE_MODES, type TraceMode } from "../../components/spectrumTraces";
import {
  AVERAGE_CHOICES,
  type AverageFrames,
  DEFAULT_AVERAGE,
} from "../../components/videoAverage";
import { type Colormap, sampleColormap } from "../../gl/colormap";
import { COLORMAPS } from "../../gl/waterfall";
import { TRACE_INK } from "./scopePlot";

const GRADIENT_STOPS = 8;

const TRACE_LABEL: Record<TraceMode, string> = {
  peak: "peak hold",
  average: "average",
  min: "min hold",
};

const AVERAGE_OPTIONS = AVERAGE_CHOICES.map((frames) => ({
  value: frames,
  label: frames === 1 ? "off" : String(frames),
}));

export interface ScopeSettingsProps {
  colormap: Colormap;
  onColormap: (name: Colormap) => void;
  average: AverageFrames;
  onAverage: (frames: AverageFrames) => void;
  traces: readonly TraceMode[];
  onTrace: (mode: TraceMode) => void;
  phosphor: boolean;
  onPhosphor: () => void;
  bands: boolean;
  onBands: () => void;
  range: DbWindow;
  manual: boolean;
  onRange: (range: DbWindow) => void;
  onAuto: () => void;
}

export function ScopeSettings(props: ScopeSettingsProps) {
  const changed =
    props.average !== DEFAULT_AVERAGE || props.traces.length > 0 || props.phosphor || props.manual;
  return (
    <Popover
      label={<Icon glyph={Settings2} size={12} />}
      title="Scope settings"
      triggerClass={plotButton(changed)}
      width="w-76"
      padded={false}
    >
      {() => (
        <SettingsPanel>
          <SettingsSection name="Colours">
            <div className="grid grid-cols-3 gap-1.5">
              {COLORMAPS.map((name) => (
                <Swatch
                  key={name}
                  name={name}
                  on={name === props.colormap}
                  onClick={() => props.onColormap(name)}
                />
              ))}
            </div>
          </SettingsSection>
          <SettingsSection name="Average" hint="Frames blended into the trace and waterfall">
            <Segmented
              label="Frames averaged"
              value={props.average}
              options={AVERAGE_OPTIONS}
              onChange={props.onAverage}
              fill
            />
          </SettingsSection>
          <SettingsSection name="Traces">
            <div className="grid grid-cols-2 gap-1.5">
              {TRACE_MODES.map((mode) => (
                <ToggleChip
                  key={mode}
                  label={TRACE_LABEL[mode]}
                  on={props.traces.includes(mode)}
                  onClick={() => props.onTrace(mode)}
                >
                  <TraceSample>
                    <span
                      className="h-0.5 w-full rounded-full"
                      style={{ background: `var(--color-${TRACE_INK[mode]})` }}
                    />
                  </TraceSample>
                </ToggleChip>
              ))}
              <ToggleChip label="phosphor" on={props.phosphor} onClick={props.onPhosphor}>
                <TraceSample>
                  <span
                    className="-mx-0.5 h-full w-[calc(100%+4px)]"
                    style={{ background: gradient(props.colormap, "to top") }}
                  />
                </TraceSample>
              </ToggleChip>
            </div>
          </SettingsSection>
          <SettingsSection
            name="Band plan"
            hint="Show band allocations above the trace"
            aside={<Switch label="Band plan" checked={props.bands} onChange={props.onBands} />}
          />
          <SettingsSection
            name="Levels"
            hint="dBFS range the waterfall colours span"
            aside={
              <span className="flex items-center gap-2 font-mono text-[10.5px] text-ink-faint">
                auto
                <Switch
                  label="Automatic levels"
                  checked={!props.manual}
                  onChange={(auto) => (auto ? props.onAuto() : props.onRange(props.range))}
                />
              </span>
            }
          >
            <Level
              name="floor"
              label="Waterfall dBFS floor"
              value={props.range.min}
              onChange={(db) => props.onRange(withFloor(props.range, db))}
            />
            <Level
              name="ceiling"
              label="Waterfall dBFS ceiling"
              value={props.range.max}
              onChange={(db) => props.onRange(withCeiling(props.range, db))}
            />
          </SettingsSection>
        </SettingsPanel>
      )}
    </Popover>
  );
}

function gradient(map: Colormap, direction: string): string {
  const stops = Array.from({ length: GRADIENT_STOPS }, (_, index) => {
    const [r, g, b] = sampleColormap(map, index / (GRADIENT_STOPS - 1));
    return `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;
  });
  return `linear-gradient(${direction}, ${stops.join(", ")})`;
}

function Swatch({ name, on, onClick }: { name: Colormap; on: boolean; onClick: () => void }) {
  return (
    <Button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={`group flex flex-col gap-1 rounded-[3px] border p-1 text-left transition-colors duration-100 ${
        on ? "border-accent-dim bg-accent/10" : "border-transparent hover:bg-panel-2"
      }`}
    >
      <span
        className="h-3 w-full rounded-[2px]"
        style={{ background: gradient(name, "to right") }}
      />
      <span
        className={`font-mono text-[10.5px] ${on ? "text-accent" : "text-ink-faint group-hover:text-ink"}`}
      >
        {name}
      </span>
    </Button>
  );
}

function TraceSample({ children }: { children: ReactNode }) {
  return (
    <span className="flex h-3.5 w-5 shrink-0 items-center overflow-hidden rounded-[3px] bg-plot-bg px-0.5">
      {children}
    </span>
  );
}

function Level({
  name,
  label,
  value,
  onChange,
}: {
  name: string;
  label: string;
  value: number;
  onChange: (db: number) => void;
}) {
  return (
    <div className="flex items-center gap-3">
      <span className="w-12 shrink-0 font-mono text-[10.5px] text-ink-faint">{name}</span>
      <Slider
        label={label}
        className="min-w-0 flex-1"
        min={DB_LIMIT.min}
        max={DB_LIMIT.max}
        step={DB_STEP}
        value={value}
        onChange={onChange}
      />
      <span className="w-14 shrink-0 text-right font-mono text-[11px] tabular-nums text-ink-dim">
        {value} dB
      </span>
    </div>
  );
}
