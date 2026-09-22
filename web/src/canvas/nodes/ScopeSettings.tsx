import { Settings2 } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "../../components/BaseControls";
import { plotButton, segmentSm } from "../../components/controls";
import { DB_LIMIT, DB_STEP, withCeiling, withFloor } from "../../components/dbRange";
import { Icon } from "../../components/Icon";
import { Popover } from "../../components/Popover";
import { Slider } from "../../components/Slider";
import { type DbWindow, TRACE_MODES, type TraceMode } from "../../components/spectrumTraces";
import { AVERAGE_CHOICES, type AverageFrames } from "../../components/videoAverage";
import { COLORMAPS, type Colormap } from "../../gl/waterfall";

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
        <div className="flex flex-col gap-2 p-2">
          <Row name="colours">
            {COLORMAPS.map((name) => (
              <Choice
                key={name}
                on={name === props.colormap}
                onClick={() => props.onColormap(name)}
              >
                {name}
              </Choice>
            ))}
          </Row>
          <Row name="avg" title="Frames blended into the live trace and waterfall">
            {AVERAGE_CHOICES.map((frames) => (
              <Choice
                key={frames}
                on={frames === props.average}
                onClick={() => props.onAverage(frames)}
              >
                {frames === 1 ? "off" : frames}
              </Choice>
            ))}
          </Row>
          <Row name="traces">
            {TRACE_MODES.map((mode) => (
              <Choice
                key={mode}
                on={props.traces.includes(mode)}
                onClick={() => props.onTrace(mode)}
              >
                {mode}
              </Choice>
            ))}
            <Choice on={props.phosphor} onClick={props.onPhosphor}>
              phosphor
            </Choice>
          </Row>
          <Row name="bands">
            <Choice on={props.bands} onClick={props.onBands}>
              {props.bands ? "on" : "off"}
            </Choice>
          </Row>
          <Row name="range" title="dB floor and ceiling the colours are spread across">
            <Choice on={!props.manual} onClick={props.onAuto}>
              auto
            </Choice>
          </Row>
          <RangeSlider
            name="min"
            label="Waterfall dB floor"
            value={props.range.min}
            onChange={(db) => props.onRange(withFloor(props.range, db))}
          />
          <RangeSlider
            name="max"
            label="Waterfall dB ceiling"
            value={props.range.max}
            onChange={(db) => props.onRange(withCeiling(props.range, db))}
          />
        </div>
      )}
    </Popover>
  );
}

function Row({ name, title, children }: { name: string; title?: string; children: ReactNode }) {
  return (
    <div className="flex items-start gap-2" title={title}>
      <span className="legend w-14 shrink-0 pt-1 text-ink-faint">{name}</span>
      <div className="flex min-w-0 flex-1 flex-wrap gap-0.5">{children}</div>
    </div>
  );
}

function Choice({
  on,
  onClick,
  children,
}: {
  on: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Button type="button" className={segmentSm(on)} aria-pressed={on} onClick={onClick}>
      {children}
    </Button>
  );
}

function RangeSlider({
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
      <span className="legend w-14 shrink-0 text-ink-faint">{name}</span>
      <Slider
        label={label}
        className="min-w-0 flex-1"
        min={DB_LIMIT.min}
        max={DB_LIMIT.max}
        step={DB_STEP}
        value={value}
        onChange={onChange}
      />
      <span className="legend w-8 shrink-0 text-right tabular-nums text-ink-dim">{value}</span>
    </div>
  );
}
