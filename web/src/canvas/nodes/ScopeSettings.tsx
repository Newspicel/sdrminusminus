import { Settings2 } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "../../components/BaseControls";
import { LABEL, plotButton } from "../../components/controls";
import { DB_LIMIT, DB_STEP, withCeiling, withFloor } from "../../components/dbRange";
import { Icon } from "../../components/Icon";
import { Popover } from "../../components/Popover";
import { Segmented } from "../../components/Segmented";
import { Slider } from "../../components/Slider";
import { Switch } from "../../components/Switch";
import { type DbWindow, TRACE_MODES, type TraceMode } from "../../components/spectrumTraces";
import { AVERAGE_CHOICES, type AverageFrames } from "../../components/videoAverage";
import { type Colormap, sampleColormap } from "../../gl/colormap";
import { COLORMAPS } from "../../gl/waterfall";
import { TRACE_INK } from "./scopePlot";

const GRADIENT_STOPS = 8;

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
  const changed = props.average > 1 || props.traces.length > 0 || props.phosphor || props.manual;
  return (
    <Popover
      label={<Icon glyph={Settings2} size={12} />}
      title="Scope settings"
      triggerClass={plotButton(changed)}
      width="w-72"
      padded={false}
    >
      {() => (
        <div className="flex flex-col divide-y divide-line">
          <Section name="Colours">
            <div className="grid grid-cols-3 gap-1">
              {COLORMAPS.map((name) => (
                <Swatch
                  key={name}
                  name={name}
                  on={name === props.colormap}
                  onClick={() => props.onColormap(name)}
                />
              ))}
            </div>
          </Section>
          <Section name="Average" title="Frames blended into the live trace and waterfall">
            <Segmented
              label="Frames averaged"
              value={props.average}
              options={AVERAGE_OPTIONS}
              onChange={props.onAverage}
              fill
            />
          </Section>
          <Section name="Traces">
            <div className="grid grid-cols-2 gap-1.5">
              {TRACE_MODES.map((mode) => (
                <TraceChip
                  key={mode}
                  name={mode}
                  on={props.traces.includes(mode)}
                  onClick={() => props.onTrace(mode)}
                >
                  <span
                    className="h-0.5 w-full rounded-full"
                    style={{ background: `var(--color-${TRACE_INK[mode]})` }}
                  />
                </TraceChip>
              ))}
              <TraceChip name="phosphor" on={props.phosphor} onClick={props.onPhosphor}>
                <span
                  className="-mx-0.5 h-full w-[calc(100%+4px)]"
                  style={{ background: gradient(props.colormap, "to top") }}
                />
              </TraceChip>
            </div>
          </Section>
          <Section
            name="Band plan"
            aside={<Switch label="Band plan" checked={props.bands} onChange={props.onBands} />}
          />
          <Section
            name="Levels"
            title="dBFS floor and ceiling the colours are spread across"
            aside={
              <span className="flex items-center gap-2">
                <span className={LABEL}>auto</span>
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
          </Section>
        </div>
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

function Section({
  name,
  title,
  aside,
  children,
}: {
  name: string;
  title?: string;
  aside?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2 px-3 py-2.5" title={title}>
      <header className="flex h-4 items-center justify-between">
        <span className={LABEL}>{name}</span>
        {aside}
      </header>
      {children}
    </section>
  );
}

function Swatch({ name, on, onClick }: { name: Colormap; on: boolean; onClick: () => void }) {
  return (
    <Button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={`group flex flex-col gap-1 rounded-[3px] p-1 text-left transition-colors duration-100 ${
        on ? "bg-accent/15" : "hover:bg-panel-2"
      }`}
    >
      <span
        className={`h-3 w-full rounded-[2px] ${on ? "ring-1 ring-accent" : ""}`}
        style={{ background: gradient(name, "to right") }}
      />
      <span
        className={`font-mono text-[10px] tracking-[0.09em] uppercase ${
          on ? "text-accent" : "text-ink-faint group-hover:text-ink"
        }`}
      >
        {name}
      </span>
    </Button>
  );
}

function TraceChip({
  name,
  on,
  onClick,
  children,
}: {
  name: string;
  on: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={`flex h-7 items-center gap-2 rounded-[3px] border px-2 font-mono text-[10px] tracking-[0.09em] uppercase transition-colors duration-100 ${
        on
          ? "border-accent bg-accent/15 text-accent"
          : "border-line text-ink-dim hover:border-line-strong hover:text-ink"
      }`}
    >
      <span className="flex h-3 w-4 shrink-0 items-center overflow-hidden rounded-[2px] bg-plot-bg px-0.5">
        {children}
      </span>
      {name}
    </Button>
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
    <div className="flex items-center gap-2">
      <span className="w-12 shrink-0 font-mono text-[10px] text-ink-faint">{name}</span>
      <Slider
        label={label}
        className="min-w-0 flex-1"
        min={DB_LIMIT.min}
        max={DB_LIMIT.max}
        step={DB_STEP}
        value={value}
        onChange={onChange}
      />
      <span className="w-12 shrink-0 text-right font-mono text-[10px] tabular-nums text-ink-dim">
        {value} dB
      </span>
    </div>
  );
}
