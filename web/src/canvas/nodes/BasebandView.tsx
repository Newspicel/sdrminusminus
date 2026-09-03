import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "../../components/BaseControls";
import {
  addConstellation,
  addEye,
  type BasebandGrid,
  clearBasebandGrid,
  createBasebandGrid,
  decayBasebandGrid,
  EYE_COMPONENTS,
  type EyeComponent,
  eyeScale,
  peakMagnitude,
  type SymbolState,
  samplesPerSymbol,
  symbolHistogram,
  symbolPhase,
  symbolStates,
  Trend,
} from "../../components/baseband";
import { plotButton, segment } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Popover } from "../../components/Popover";
import { colormapLut } from "../../components/persistence";
import { FULL_VIEW } from "../../components/spectrumView";
import type { Colormap } from "../../gl/colormap";
import { SpectrumAnalyzer } from "../../lib/dsp/fft";
import type { IqFrame, SymbolFrame } from "../../lib/frame";
import { iqHub } from "../../lib/iq";
import { symbolHub } from "../../lib/symbols";
import { token } from "../../lib/tokens";
import type { ChannelInfo } from "../../lib/types";
import { drawPlot, GridBitmap } from "./scopePlot";
import { drawHistogram, drawStates, drawTrend, type PlotInset } from "./symbolPlot";

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

const FFT_SIZE = 2048;
const GRID = 320;
const SPECTRUM_RANGE_DB = 90;
const MIN_SYMBOL_RATE = 1;
const TREND_POINTS = 240;
const SCATTER_VIEWS: readonly BasebandView[] = ["constellation", "eye"];
const HEADER_INSET = 14;
const CHROME_GAP = 6;

export function BasebandView({
  deviceSet,
  channel,
  colormap,
  label,
}: {
  deviceSet: number;
  channel: ChannelInfo;
  colormap: Colormap;
  label: ReactNode;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [frame, setFrame] = useState<IqFrame | null>(() => iqHub.latest(deviceSet, channel.id));
  const frameRef = useRef<IqFrame | null>(frame);
  const [view, setView] = useState<BasebandView>("spectrum");
  const [eyeComponent, setEyeComponent] = useState<EyeComponent>("frequency");
  const [symbolRate, setSymbolRate] = useState(4800);
  const [decimate, setDecimate] = useState(false);

  const [symbols, setSymbols] = useState<SymbolFrame | null>(() =>
    symbolHub.latest(deviceSet, channel.id),
  );
  const symbolsRef = useRef<SymbolFrame | null>(symbols);
  const merRef = useRef(new Trend(TREND_POINTS));
  const marginRef = useRef(new Trend(TREND_POINTS));
  const driftRef = useRef(new Trend(TREND_POINTS));

  const chromeRef = useRef<HTMLDivElement>(null);
  const insetRef = useRef<PlotInset>({ top: HEADER_INSET, bottom: 0 });
  const gridRef = useRef<BasebandGrid | null>(null);
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
            addConstellation(
              grid,
              burst.samples,
              peakMagnitude(burst.samples),
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
        addConstellation(grid, paired(block), referenceScale(block));
      }
      seen += 1;
      if (seen === 1 || seen % 4 === 0) {
        setSymbols(block);
      }
    });
  }, [deviceSet, channel.id]);

  useEffect(() => {
    const chrome = chromeRef.current;
    if (chrome === null) {
      return;
    }
    const measure = () => {
      insetRef.current = { top: HEADER_INSET, bottom: chrome.offsetHeight + CHROME_GAP };
    };
    const observer = new ResizeObserver(measure);
    observer.observe(chrome);
    measure();
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    let raf = 0;
    const loop = () => {
      draw(
        canvasRef.current,
        frameRef.current,
        symbolsRef.current,
        settingsRef.current,
        gridRef.current,
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

  return (
    <div className="relative flex h-full min-h-0 flex-col overflow-hidden bg-plot-bg">
      <canvas ref={canvasRef} className="h-full w-full min-h-0 flex-1" />

      <div className="pointer-events-none absolute inset-0 flex flex-col justify-between p-1.5">
        <span className="legend self-end text-right whitespace-pre text-plot-ink-dim">
          {readout(view, frame, symbols, period)}
        </span>
        <div
          ref={chromeRef}
          data-plot-chrome
          className="pointer-events-auto flex max-w-full flex-wrap items-center gap-1 self-start rounded-[3px] bg-plot-bg/85 p-0.5"
        >
          {BASEBAND_VIEWS.map((name) => (
            <Button
              key={name}
              type="button"
              className={plotButton(view === name)}
              aria-pressed={view === name}
              onClick={() => setView(name)}
            >
              {name}
            </Button>
          ))}
          {view === "eye" && (
            <Popover
              label={eyeComponent}
              triggerClass={plotButton(false)}
              width="w-auto min-w-[var(--anchor-width)]"
              padded={false}
            >
              {(close) => (
                <div className="flex flex-col p-0.5">
                  {EYE_COMPONENTS.map((name) => (
                    <Button
                      key={name}
                      type="button"
                      className={`${segment(name === eyeComponent)} justify-start`}
                      onClick={() => {
                        setEyeComponent(name);
                        close();
                      }}
                    >
                      {name}
                    </Button>
                  ))}
                </div>
              )}
            </Popover>
          )}
          {view === "constellation" && symbols === null && (
            <Button
              type="button"
              className={plotButton(decimate)}
              aria-pressed={decimate}
              onClick={() => setDecimate(!decimate)}
            >
              symbols
            </Button>
          )}
          {(view === "eye" ||
            (symbols === null &&
              (view === "levels" || (view === "constellation" && decimate)))) && (
            <>
              <NumberField
                label="Symbol rate"
                className="w-20"
                value={symbolRate}
                min={MIN_SYMBOL_RATE}
                max={nyquist}
                step={100}
                onCommit={setSymbolRate}
              />
              <span className="legend text-plot-ink-dim">Bd</span>
            </>
          )}
        </div>
      </div>

      <span className="pointer-events-none absolute inset-x-0 top-0 p-1.5 text-plot-ink-dim legend">
        {label}
      </span>

      {waiting(view, frame, symbols) !== null && (
        <p className="pointer-events-none absolute inset-0 flex items-center justify-center pb-12 text-sm text-plot-ink-dim">
          {waiting(view, frame, symbols)}
        </p>
      )}
    </div>
  );
}

export function readout(
  view: BasebandView,
  frame: IqFrame | null,
  block: SymbolFrame | null,
  period: number,
): string {
  if (SCATTER_VIEWS.includes(view) && view !== "constellation") {
    return frame === null ? "" : formatReadout(frame, view, period);
  }
  if (view === "spectrum") {
    return frame === null ? "" : formatReadout(frame, view, period);
  }
  if (block !== null) {
    return formatMeasurement(block);
  }
  return frame === null ? "" : formatReadout(frame, view, period);
}

export function formatMeasurement(block: SymbolFrame): string {
  const rate =
    block.symbolRate >= 1000
      ? `${(block.symbolRate / 1000).toFixed(2)} kBd`
      : `${block.symbolRate.toFixed(2)} Bd`;
  const mer = block.merDb >= 99 ? "clean" : `${block.merDb.toFixed(1)} dB MER`;
  return `${rate}   ${(block.evm * 100).toFixed(1)}% EVM   ${mer}   ×${block.margin.toFixed(2)} margin   ${block.freqErrorHz >= 0 ? "+" : ""}${block.freqErrorHz.toFixed(0)} Hz`;
}

export function waiting(
  view: BasebandView,
  frame: IqFrame | null,
  block: SymbolFrame | null,
): string | null {
  if (view === "quality" || view === "drift" || view === "states") {
    return block === null ? "This channel's decoder does not report symbols." : null;
  }
  if (frame === null && block === null) {
    return "Waiting for the first burst…";
  }
  return null;
}

function formatReadout(frame: IqFrame, view: BasebandView, period: number): string {
  const rate =
    frame.sampleRate >= 1e6
      ? `${(frame.sampleRate / 1e6).toFixed(3)} MSa/s`
      : `${(frame.sampleRate / 1e3).toFixed(1)} kSa/s`;
  const centre = `${(frame.centerHz / 1e6).toFixed(4)} MHz`;
  if (view === "spectrum") {
    return `${centre}   ${rate}`;
  }
  return `${centre}   ${rate}   ${period.toFixed(2)} Sa/sym`;
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
  grid: BasebandGrid | null,
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
    });
    return;
  }
  drawScatter(canvas, grid, bitmapRef, colormap, view);
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

function drawScatter(
  canvas: HTMLCanvasElement,
  grid: BasebandGrid | null,
  bitmapRef: { current: GridBitmap | null },
  colormap: Colormap,
  view: BasebandView,
): void {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0 || grid === null) {
    return;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return;
  }
  ctx.clearRect(0, 0, width, height);

  const side = view === "constellation" ? Math.min(width, height) : 0;
  const box =
    side > 0
      ? { x: (width - side) / 2, y: (height - side) / 2, w: side, h: side }
      : { x: 0, y: 0, w: width, h: height };

  ctx.strokeStyle = token("plot-grid");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(box.x, Math.round(box.y + box.h / 2) + 0.5);
  ctx.lineTo(box.x + box.w, Math.round(box.y + box.h / 2) + 0.5);
  if (view === "constellation") {
    ctx.moveTo(Math.round(box.x + box.w / 2) + 0.5, box.y);
    ctx.lineTo(Math.round(box.x + box.w / 2) + 0.5, box.y + box.h);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(box.x + box.w / 2, box.y + box.h / 2, box.w / 2, 0, Math.PI * 2);
  }
  ctx.stroke();

  const bitmap = bitmapRef.current ?? new GridBitmap(grid.width, grid.height);
  bitmapRef.current = bitmap;
  bitmap.blit(ctx, box, (out) => recolour(grid, colormap, out));
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
