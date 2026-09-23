import { Settings2 } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "../../components/BaseControls";
import {
  addConstellation,
  addEye,
  type BasebandGrid,
  clearBasebandGrid,
  createBasebandGrid,
  decayBasebandGrid,
  type EyeComponent,
  eyeScale,
  iqScale,
  type SymbolState,
  samplesPerSymbol,
  symbolHistogram,
  symbolPhase,
  symbolStates,
  Trend,
} from "../../components/baseband";
import { ICON_BTN_SM, TAB_BAR, tab } from "../../components/controls";
import { Icon } from "../../components/Icon";
import { NumberField } from "../../components/NumberField";
import { Popover } from "../../components/Popover";
import { colormapLut } from "../../components/persistence";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Segmented } from "../../components/Segmented";
import { SettingsPanel, SettingsSection } from "../../components/SettingsPanel";
import { Switch } from "../../components/Switch";
import { FULL_VIEW } from "../../components/spectrumView";
import type { Colormap } from "../../gl/colormap";
import { SpectrumAnalyzer } from "../../lib/dsp/fft";
import type { IqFrame, SymbolFrame } from "../../lib/frame";
import { iqHub } from "../../lib/iq";
import { symbolHub } from "../../lib/symbols";
import { token } from "../../lib/tokens";
import type { ChannelInfo } from "../../lib/types";
import { measurements } from "./basebandMeasure";
import { drawPlot, GridBitmap, PLOT_FONT, prepareCanvas } from "./scopePlot";
import {
  drawGraticule,
  drawHistogram,
  drawStates,
  drawTrend,
  type PlotBox,
  type PlotInset,
  plotLabel,
} from "./symbolPlot";

export const BASEBAND_VIEWS = [
  "spectrum",
  "constellation",
  "eye",
  "levels",
  "states",
  "quality",
  "drift",
] as const;
export type BasebandView = (typeof BASEBAND_VIEWS)[number];

const EYE_OPTIONS = [
  { value: "i", label: "I" },
  { value: "q", label: "Q" },
  { value: "frequency", label: "freq" },
] as const satisfies readonly { value: EyeComponent; label: string }[];

const FFT_SIZE = 2048;
const GRID = 320;
const SPECTRUM_RANGE_DB = 90;
const MIN_SYMBOL_RATE = 1;
const TREND_POINTS = 240;
const SCATTER_VIEWS: readonly BasebandView[] = ["constellation", "eye"];
const SIGNAL_VIEWS: readonly BasebandView[] = ["spectrum", "constellation", "eye", "levels"];
const SYMBOL_VIEWS: readonly BasebandView[] = ["states", "quality", "drift"];
const MEASURE_COLUMNS =
  "grid-cols-[repeat(3,auto_auto)] justify-start gap-x-4 @[36rem]:grid-cols-[repeat(4,auto_auto)] @[60rem]:grid-cols-[repeat(8,auto_auto)]";
const SCATTER_PAD = 12;
const PLOT_INSET: PlotInset = { top: 10, bottom: 4 };

export function BasebandView({
  deviceSet,
  channel,
  colormap,
}: {
  deviceSet: number;
  channel: ChannelInfo;
  colormap: Colormap;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [frame, setFrame] = useState<IqFrame | null>(() => iqHub.latest(deviceSet, channel.id));
  const frameRef = useRef<IqFrame | null>(frame);
  const [view, setView] = useState<BasebandView>("spectrum");
  const [eyeComponent, setEyeComponent] = useState<EyeComponent>("frequency");
  const [symbolRate, setSymbolRate] = useState(4800);
  const [decimate, setDecimate] = useState(true);

  const [symbols, setSymbols] = useState<SymbolFrame | null>(() =>
    symbolHub.latest(deviceSet, channel.id),
  );
  const symbolsRef = useRef<SymbolFrame | null>(symbols);
  const merRef = useRef(new Trend(TREND_POINTS));
  const marginRef = useRef(new Trend(TREND_POINTS));
  const driftRef = useRef(new Trend(TREND_POINTS));

  const insetRef = useRef<PlotInset>(PLOT_INSET);
  const gridRef = useRef<BasebandGrid | null>(null);
  const scaleRef = useRef(1);
  const bitmapRef = useRef<GridBitmap | null>(null);
  const analyzerRef = useRef<SpectrumAnalyzer | null>(null);
  const dbRef = useRef<Float32Array | null>(null);
  const settingsRef = useRef({ view, eyeComponent, symbolRate, decimate });
  useLayoutEffect(() => {
    settingsRef.current = { view, eyeComponent, symbolRate, decimate };
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: the clear is the effect
  useEffect(() => {
    const grid = gridRef.current;
    if (grid !== null) {
      clearBasebandGrid(grid);
      bitmapRef.current?.invalidate();
    }
    // oxlint-disable-next-line react/exhaustive-effect-dependencies -- the settings are what the clear reacts to
  }, [view, eyeComponent, symbolRate, decimate]);

  useEffect(() => {
    let seen = 0;
    return iqHub.subscribe(deviceSet, channel.id, (burst) => {
      frameRef.current = burst;
      const {
        view: mode,
        eyeComponent: rail,
        symbolRate: rate,
        decimate: sparse,
      } = settingsRef.current;
      if (SCATTER_VIEWS.includes(mode)) {
        const grid = gridRef.current ?? createBasebandGrid(GRID, GRID);
        gridRef.current = grid;
        decayBasebandGrid(grid);
        bitmapRef.current?.invalidate();
        const period = samplesPerSymbol(burst.sampleRate, rate);
        if (mode === "constellation") {
          if (symbolsRef.current === null) {
            const step = sparse ? Math.round(period) : 1;
            scaleRef.current = iqScale(burst.samples);
            addConstellation(
              grid,
              burst.samples,
              scaleRef.current,
              step,
              sparse ? symbolPhase(burst.samples, period) : 0,
            );
          }
        } else {
          addEye(grid, burst.samples, period, rail, eyeScale(burst.samples, rail));
        }
      }
      seen += 1;
      if (seen === 1 || seen % 10 === 0) {
        setFrame(burst);
      }
    });
  }, [deviceSet, channel.id]);

  useEffect(() => {
    let seen = 0;
    merRef.current.clear();
    marginRef.current.clear();
    driftRef.current.clear();
    return symbolHub.subscribe(deviceSet, channel.id, (block) => {
      symbolsRef.current = block;
      merRef.current.push(block.merDb);
      marginRef.current.push(block.margin);
      driftRef.current.push(block.freqErrorHz);
      const { view: mode } = settingsRef.current;
      if (mode === "constellation") {
        const grid = gridRef.current ?? createBasebandGrid(GRID, GRID);
        gridRef.current = grid;
        decayBasebandGrid(grid);
        bitmapRef.current?.invalidate();
        scaleRef.current = referenceScale(block);
        addConstellation(grid, paired(block), scaleRef.current);
      }
      seen += 1;
      if (seen === 1 || seen % 4 === 0) {
        setSymbols(block);
      }
    });
  }, [deviceSet, channel.id]);

  useEffect(() => {
    let raf = 0;
    const loop = () => {
      draw(
        canvasRef.current,
        frameRef.current,
        symbolsRef.current,
        settingsRef.current,
        { grid: gridRef.current, scale: scaleRef.current },
        bitmapRef,
        colormap,
        analyzerRef,
        dbRef,
        { mer: merRef.current, margin: marginRef.current, drift: driftRef.current },
        insetRef.current,
      );
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [colormap]);

  const nyquist = frame === null ? Number.POSITIVE_INFINITY : frame.sampleRate / 2;
  const period = frame === null ? 0 : samplesPerSymbol(frame.sampleRate, symbolRate);

  const shown = symbols === null && SYMBOL_VIEWS.includes(view) ? "spectrum" : view;
  const hint = waiting(shown, frame, symbols);
  const rows = measurements(shown, frame, symbols, period);

  return (
    <div className="@container flex h-full min-h-0 flex-col overflow-hidden">
      <div data-plot-chrome className={`${TAB_BAR} shrink-0 pr-1`}>
        <div
          role="group"
          aria-label="Baseband view"
          className="flex min-w-0 flex-wrap items-stretch"
        >
          {SIGNAL_VIEWS.map((name) => (
            <ViewTab key={name} name={name} active={shown === name} onPick={setView} />
          ))}
          {symbols !== null && (
            <>
              <span aria-hidden className="mx-1 my-2 w-px bg-line" />
              {SYMBOL_VIEWS.map((name) => (
                <ViewTab key={name} name={name} active={shown === name} onPick={setView} />
              ))}
            </>
          )}
        </div>
        <span className="ml-auto flex items-center">
          <ViewOptions
            view={shown}
            symbols={symbols !== null}
            eyeComponent={eyeComponent}
            onEyeComponent={setEyeComponent}
            decimate={decimate}
            onDecimate={setDecimate}
            symbolRate={symbolRate}
            onSymbolRate={setSymbolRate}
            nyquist={nyquist}
          />
        </span>
      </div>
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="relative min-h-0 min-w-0 flex-1 bg-plot-bg">
          <canvas ref={canvasRef} className="absolute inset-0 h-full w-full" />
          {hint !== null && (
            <span className="legend pointer-events-none absolute inset-0 flex items-center justify-center text-plot-ink-dim">
              {hint}
            </span>
          )}
        </div>
        {rows.length > 0 && (
          <Readout
            separated={false}
            columns={MEASURE_COLUMNS}
            className="shrink-0 border-t border-line bg-panel"
          >
            {rows.map((row) => (
              <ReadoutRow key={row.label} label={row.label} title={row.hint}>
                {row.value}
              </ReadoutRow>
            ))}
          </Readout>
        )}
      </div>
    </div>
  );
}

function ViewTab({
  name,
  active,
  onPick,
}: {
  name: BasebandView;
  active: boolean;
  onPick: (view: BasebandView) => void;
}) {
  return (
    <Button
      type="button"
      className={tab(active)}
      aria-pressed={active}
      onClick={() => onPick(name)}
    >
      {name}
    </Button>
  );
}

function ViewOptions({
  view,
  symbols,
  eyeComponent,
  onEyeComponent,
  decimate,
  onDecimate,
  symbolRate,
  onSymbolRate,
  nyquist,
}: {
  view: BasebandView;
  symbols: boolean;
  eyeComponent: EyeComponent;
  onEyeComponent: (component: EyeComponent) => void;
  decimate: boolean;
  onDecimate: (on: boolean) => void;
  symbolRate: number;
  onSymbolRate: (rate: number) => void;
  nyquist: number;
}) {
  const needsRate =
    view === "eye" || (!symbols && (view === "levels" || (view === "constellation" && decimate)));
  const decimates = view === "constellation" && !symbols;
  if (view !== "eye" && !decimates && !needsRate) {
    return null;
  }
  return (
    <Popover
      label={<Icon glyph={Settings2} size={12} />}
      title="View settings"
      triggerClass={ICON_BTN_SM}
      width="w-64"
      padded={false}
    >
      {() => (
        <SettingsPanel>
          {view === "eye" && (
            <SettingsSection name="Trace">
              <Segmented
                label="Eye trace"
                value={eyeComponent}
                options={EYE_OPTIONS}
                onChange={onEyeComponent}
                fill
              />
            </SettingsSection>
          )}
          {decimates && (
            <SettingsSection
              name="Symbols only"
              hint="Plot one point per symbol instead of every sample"
              aside={<Switch label="Symbols only" checked={decimate} onChange={onDecimate} />}
            />
          )}
          {needsRate && (
            <SettingsSection
              name="Symbol rate"
              aside={
                <NumberField
                  label="Symbol rate"
                  className="w-24"
                  value={symbolRate}
                  min={MIN_SYMBOL_RATE}
                  max={nyquist}
                  step={100}
                  onCommit={onSymbolRate}
                  unit="Bd"
                />
              }
            />
          )}
        </SettingsPanel>
      )}
    </Popover>
  );
}

export function waiting(
  view: BasebandView,
  frame: IqFrame | null,
  block: SymbolFrame | null,
): string | null {
  if (view === "quality" || view === "drift" || view === "states") {
    return block === null ? "This decoder reports no symbols" : null;
  }
  if (frame === null && block === null) {
    return "No burst yet";
  }
  return null;
}

interface Trends {
  mer: Trend;
  margin: Trend;
  drift: Trend;
}

export function paired(block: SymbolFrame): Float32Array {
  if (block.plane === "complex") {
    return block.symbols;
  }
  const out = new Float32Array(block.symbols.length * 2);
  for (let i = 0; i < block.symbols.length; i++) {
    out[i * 2] = block.symbols[i] ?? 0;
  }
  return out;
}

export function referenceScale(block: SymbolFrame): number {
  let peak = 0;
  if (block.plane === "complex") {
    for (let i = 0; i + 1 < block.reference.length; i += 2) {
      peak = Math.max(peak, Math.hypot(block.reference[i] ?? 0, block.reference[i + 1] ?? 0));
    }
  } else {
    for (const level of block.reference) {
      peak = Math.max(peak, Math.abs(level));
    }
  }
  return peak > 0 ? peak * 1.4 : 1;
}

function draw(
  canvas: HTMLCanvasElement | null,
  frame: IqFrame | null,
  block: SymbolFrame | null,
  settings: { view: BasebandView; eyeComponent: EyeComponent; symbolRate: number },
  scatter: Scatter,
  bitmapRef: { current: GridBitmap | null },
  colormap: Colormap,
  analyzerRef: { current: SpectrumAnalyzer | null },
  dbRef: { current: Float32Array | null },
  trends: Trends,
  inset: PlotInset,
): void {
  if (canvas === null) {
    return;
  }
  const view = settings.view;
  if (view === "quality") {
    drawTrend(
      canvas,
      [
        { trend: trends.mer, colour: token("plot-trace"), label: "MER dB" },
        { trend: trends.margin, colour: token("plot-hold"), label: "margin" },
      ],
      "per block",
      false,
      inset,
    );
    return;
  }
  if (view === "drift") {
    drawTrend(
      canvas,
      [{ trend: trends.drift, colour: token("plot-trace"), label: "carrier" }],
      "Hz",
      true,
      inset,
    );
    return;
  }
  if (view === "states") {
    drawStates(canvas, block === null ? [] : statesOf(block), block?.plane !== "complex", inset);
    return;
  }
  if (view === "levels") {
    drawLevels(canvas, frame, block, settings, inset);
    return;
  }
  if (frame === null) {
    return;
  }
  if (view === "spectrum") {
    const analyzer = analyzerRef.current ?? new SpectrumAnalyzer(FFT_SIZE);
    analyzerRef.current = analyzer;
    const db = analyzer.powerDb(frame.samples, dbRef.current ?? new Float32Array(FFT_SIZE));
    dbRef.current = db;
    let peak = Number.NEGATIVE_INFINITY;
    for (const value of db) {
      if (value > peak) {
        peak = value;
      }
    }
    const top = Math.ceil(peak / 10) * 10;
    drawPlot(canvas, {
      frame: { centerHz: frame.centerHz, spanHz: frame.sampleRate, db },
      view: FULL_VIEW,
      window: { min: top - SPECTRUM_RANGE_DB, max: top },
      traces: [],
      density: null,
      offsetAxis: true,
    });
    return;
  }
  drawScatter(canvas, scatter, bitmapRef, colormap, view, settings.eyeComponent);
}

function drawLevels(
  canvas: HTMLCanvasElement,
  frame: IqFrame | null,
  block: SymbolFrame | null,
  settings: { symbolRate: number },
  inset: PlotInset,
): void {
  if (block !== null) {
    const scale = referenceScale(block);
    const stride = block.plane === "complex" ? 2 : 1;
    drawHistogram(
      canvas,
      symbolHistogram(block.symbols, stride, scale),
      [...block.reference],
      scale,
      inset,
    );
    return;
  }
  if (frame === null) {
    return;
  }
  const period = samplesPerSymbol(frame.sampleRate, settings.symbolRate);
  const rail = discriminator(frame.samples, period, symbolPhase(frame.samples, period));
  drawHistogram(canvas, symbolHistogram(rail, 1, 1), [], 1, inset);
}

const stateCache = new WeakMap<SymbolFrame, SymbolState[]>();

function statesOf(block: SymbolFrame): SymbolState[] {
  const hit = stateCache.get(block);
  if (hit !== undefined) {
    return hit;
  }
  const states = symbolStates(block);
  stateCache.set(block, states);
  return states;
}

export function discriminator(samples: Float32Array, period: number, offset: number): Float32Array {
  const count = samples.length >> 1;
  const step = Math.max(1, Math.round(period));
  const out: number[] = [];
  for (let i = Math.max(1, offset); i < count; i += step) {
    const re = samples[i * 2] ?? 0;
    const im = samples[i * 2 + 1] ?? 0;
    const pr = samples[i * 2 - 2] ?? 0;
    const pi = samples[i * 2 - 1] ?? 0;
    out.push(Math.atan2(im * pr - re * pi, re * pr + im * pi) / Math.PI);
  }
  return Float32Array.from(out);
}

interface Scatter {
  grid: BasebandGrid | null;
  scale: number;
}

function drawScatter(
  canvas: HTMLCanvasElement,
  { grid, scale }: Scatter,
  bitmapRef: { current: GridBitmap | null },
  colormap: Colormap,
  view: BasebandView,
  eyeComponent: EyeComponent,
): void {
  const prepared = prepareCanvas(canvas);
  if (prepared === null || grid === null) {
    return;
  }
  const { ctx, width, height } = prepared;
  ctx.font = PLOT_FONT;
  ctx.lineWidth = 1;
  const inner = { w: width - 2 * SCATTER_PAD, h: height - 2 * SCATTER_PAD };
  if (inner.w <= 0 || inner.h <= 0) {
    return;
  }
  const constellation = view === "constellation";
  const side = Math.min(inner.w, inner.h);
  const box = constellation
    ? { x: (width - side) / 2, y: (height - side) / 2, w: side, h: side }
    : { x: SCATTER_PAD, y: SCATTER_PAD, w: inner.w, h: inner.h };

  drawGraticule(ctx, box, constellation ? 8 : 8, constellation ? 8 : 6);
  if (constellation) {
    ctx.strokeStyle = token("plot-ink-dim");
    ctx.globalAlpha = 0.35;
    ctx.setLineDash([2, 4]);
    ctx.beginPath();
    ctx.arc(box.x + box.w / 2, box.y + box.h / 2, box.w / 2, 0, Math.PI * 2);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;
  }

  const bitmap = bitmapRef.current ?? new GridBitmap(grid.width, grid.height);
  bitmapRef.current = bitmap;
  bitmap.blit(ctx, box, (out) => recolour(grid, colormap, out));

  if (constellation) {
    plotLabel(ctx, "I", box.x + box.w - 5, box.y + box.h / 2 - 6, "right");
    plotLabel(ctx, "Q", box.x + box.w / 2 + 6, box.y + 12);
    drawIqTicks(ctx, box, scale);
  } else {
    plotLabel(
      ctx,
      eyeComponent === "frequency" ? "freq" : eyeComponent.toUpperCase(),
      box.x + 5,
      box.y + 12,
    );
    plotLabel(ctx, "2 symbols", box.x + box.w - 5, box.y + box.h - 5, "right");
  }
}

function drawIqTicks(ctx: CanvasRenderingContext2D, box: PlotBox, scale: number): void {
  const half = scale / 2;
  const bottom = box.y + box.h - 5;
  plotLabel(ctx, tickLabel(-half), box.x + box.w / 4, bottom, "center");
  plotLabel(ctx, tickLabel(half), box.x + (box.w * 3) / 4, bottom, "center");
  plotLabel(ctx, tickLabel(half), box.x + 5, box.y + box.h / 4 + 4);
  plotLabel(ctx, tickLabel(-half), box.x + 5, box.y + (box.h * 3) / 4 + 4);
}

export function tickLabel(value: number): string {
  return Number(value.toPrecision(2)).toString();
}

function recolour(grid: BasebandGrid, colormap: Colormap, out: Uint8ClampedArray): void {
  const lut = colormapLut(colormap);
  for (let i = 0; i < grid.cells.length; i++) {
    const value = grid.cells[i] ?? 0;
    const at = i * 4;
    if (value <= 0) {
      out[at + 3] = 0;
      continue;
    }
    const entry = Math.min(255, Math.round(value * 255)) * 3;
    out[at] = lut[entry] ?? 0;
    out[at + 1] = lut[entry + 1] ?? 0;
    out[at + 2] = lut[entry + 2] ?? 0;
    out[at + 3] = Math.min(255, Math.round(40 + value * 215));
  }
}
