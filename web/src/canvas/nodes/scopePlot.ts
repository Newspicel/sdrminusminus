import { formatMhz } from "../../components/format";
import {
  addDensity,
  clearDensity,
  createDensity,
  type DensityGrid,
  decayDensity,
  densityToImage,
} from "../../components/persistence";
import { type DbWindow, type TraceMode, traceUnit } from "../../components/spectrumTraces";
import {
  decibelTicks,
  frequencyTicks,
  type SpectrumView,
  spanToOffset,
  spanToView,
  viewToSpan,
  viewWidth,
} from "../../components/spectrumView";
import type { Colormap } from "../../gl/colormap";
import { pixelRatio, zoomOf } from "../../gl/raster";
import { token } from "../../lib/tokens";

export const AXIS_H = 16;
const TRACE_FILL_ALPHA = 0.2;

const TRACE_INK: Record<TraceMode, string> = {
  peak: "plot-hold",
  average: "plot-ink",
  min: "plot-ink-dim",
};

export interface PlotTrace {
  mode: TraceMode;
  db: Float32Array;
}

export interface PlotFrame {
  centerHz: number;
  spanHz: number;
  db: Float32Array;
}

export interface PlotOptions {
  frame: PlotFrame | null;
  view: SpectrumView;
  window: DbWindow;
  traces: readonly PlotTrace[];
  density: DensityLayer | null;
  cursor?: number | null;
}

export function readoutAt(
  frame: PlotFrame,
  view: SpectrumView,
  at: number,
): { hz: number; db: number } | null {
  if (at < 0 || at > 1 || frame.db.length === 0 || !(frame.spanHz > 0)) {
    return null;
  }
  const fraction = viewToSpan(view, at);
  const hz = frame.centerHz + spanToOffset(fraction, frame.spanHz);
  const index = Math.min(
    frame.db.length - 1,
    Math.max(0, Math.round(fraction * (frame.db.length - 1))),
  );
  return { hz, db: frame.db[index] ?? Number.NEGATIVE_INFINITY };
}

export class GridBitmap {
  private readonly canvas = document.createElement("canvas");
  private readonly ctx: CanvasRenderingContext2D | null;
  private readonly image: ImageData;
  private dirty = true;

  constructor(
    readonly width: number,
    readonly height: number,
  ) {
    this.canvas.width = width;
    this.canvas.height = height;
    this.ctx = this.canvas.getContext("2d");
    this.image = new ImageData(width, height);
  }

  invalidate(): void {
    this.dirty = true;
  }

  blit(
    ctx: CanvasRenderingContext2D,
    box: { x: number; y: number; w: number; h: number },
    recolour: (out: Uint8ClampedArray) => void,
  ): void {
    if (this.ctx === null) {
      return;
    }
    if (this.dirty) {
      recolour(this.image.data);
      this.ctx.putImageData(this.image, 0, 0);
      this.dirty = false;
    }
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(this.canvas, box.x, box.y, box.w, box.h);
  }
}

export class DensityLayer {
  readonly grid: DensityGrid = createDensity();
  private readonly bitmap: GridBitmap;
  private colormap: Colormap;

  constructor(colormap: Colormap) {
    this.colormap = colormap;
    this.bitmap = new GridBitmap(this.grid.width, this.grid.height);
  }

  setColormap(name: Colormap): void {
    if (name !== this.colormap) {
      this.colormap = name;
      this.bitmap.invalidate();
    }
  }

  add(db: Float32Array, view: SpectrumView, window: DbWindow): void {
    decayDensity(this.grid);
    addDensity(this.grid, db, view, window);
    this.bitmap.invalidate();
  }

  clear(): void {
    clearDensity(this.grid);
    this.bitmap.invalidate();
  }

  blit(ctx: CanvasRenderingContext2D, width: number, height: number): void {
    this.bitmap.blit(ctx, { x: 0, y: 0, w: width, h: height }, (out) =>
      densityToImage(this.grid, this.colormap, out),
    );
  }
}

function prepare(
  canvas: HTMLCanvasElement,
): { ctx: CanvasRenderingContext2D; width: number; height: number } | null {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0) {
    return null;
  }
  const rect = canvas.getBoundingClientRect();
  const ratio = pixelRatio(window.devicePixelRatio, zoomOf(rect.width, width));
  const w = Math.round(width * ratio);
  const h = Math.round(height * ratio);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return null;
  }
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.clearRect(0, 0, width, height);
  return { ctx, width, height };
}

export function drawPlot(canvas: HTMLCanvasElement | null, options: PlotOptions): void {
  if (canvas === null) {
    return;
  }
  const prepared = prepare(canvas);
  if (prepared === null) {
    return;
  }
  const { ctx, width, height } = prepared;
  const { frame, view, window: dbWindow } = options;
  if (frame === null || frame.db.length < 2 || !(dbWindow.max > dbWindow.min)) {
    return;
  }
  const plotH = Math.max(1, height - AXIS_H);

  options.density?.blit(ctx, width, plotH);

  ctx.font = '10px ui-monospace, "SF Mono", Menlo, monospace';
  ctx.textBaseline = "middle";
  ctx.lineWidth = 1;
  drawGrid(ctx, frame, view, dbWindow, width, height, plotH);

  for (const trace of options.traces) {
    ctx.strokeStyle = token(TRACE_INK[trace.mode]);
    ctx.lineWidth = 1;
    tracePath(ctx, trace.db, view, width, plotH, dbWindow);
    ctx.stroke();
  }

  ctx.strokeStyle = token("plot-trace");
  ctx.lineWidth = 1.25;
  ctx.lineJoin = "round";
  tracePath(ctx, frame.db, view, width, plotH, dbWindow);
  ctx.stroke();
  ctx.lineTo(width, plotH);
  ctx.lineTo(0, plotH);
  ctx.closePath();
  ctx.fillStyle = token("plot-trace");
  ctx.globalAlpha = TRACE_FILL_ALPHA;
  ctx.fill();
  ctx.globalAlpha = 1;

  const cursor = options.cursor ?? null;
  if (cursor !== null) {
    drawCursor(ctx, frame, view, cursor, width, plotH);
  }
}

function drawCursor(
  ctx: CanvasRenderingContext2D,
  frame: PlotFrame,
  view: SpectrumView,
  at: number,
  width: number,
  plotH: number,
): void {
  const readout = readoutAt(frame, view, at);
  if (readout === null) {
    return;
  }
  const x = Math.round(at * width) + 0.5;
  ctx.strokeStyle = token("plot-ink-dim");
  ctx.globalAlpha = 0.9;
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, plotH);
  ctx.stroke();
  ctx.globalAlpha = 1;
  const level = Number.isFinite(readout.db) ? `  ${readout.db.toFixed(1)} dB` : "";
  const text = `${formatMhz(readout.hz)}${level}`;
  const w = ctx.measureText(text).width + 8;
  const left = x + 6 + w > width ? x - 6 - w : x + 6;
  ctx.fillStyle = token("plot-bg");
  ctx.globalAlpha = 0.85;
  ctx.fillRect(left, 4, w, 14);
  ctx.globalAlpha = 1;
  ctx.fillStyle = token("plot-ink");
  ctx.textAlign = "left";
  ctx.fillText(text, left + 4, 11);
}

function drawGrid(
  ctx: CanvasRenderingContext2D,
  frame: PlotFrame,
  view: SpectrumView,
  dbWindow: DbWindow,
  width: number,
  height: number,
  plotH: number,
): void {
  ctx.strokeStyle = token("plot-grid");
  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = "left";
  for (const db of decibelTicks(dbWindow.min, dbWindow.max, 4)) {
    const y = Math.round(plotH * (1 - traceUnit(db, dbWindow))) + 0.5;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(width, y);
    ctx.stroke();
    if (y > 12 && y < plotH - 4) {
      ctx.fillText(db.toFixed(0), 4, y - 7);
    }
  }

  const visible = frame.spanHz * viewWidth(view);
  const ticks = frequencyTicks(
    frame.centerHz,
    frame.spanHz,
    view,
    Math.max(2, Math.floor(width / 110)),
  );
  ctx.textAlign = "center";
  for (const tick of ticks) {
    const x = Math.round(tick.at * width) + 0.5;
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, plotH);
    ctx.stroke();
    ctx.fillText(formatTick(tick.hz, visible), x, height - AXIS_H / 2);
  }
  ctx.textAlign = "left";

  const centerAt = spanToView(view, 0.5);
  if (centerAt >= 0 && centerAt <= 1) {
    const x = Math.round(centerAt * width) + 0.5;
    ctx.strokeStyle = token("plot-ink-dim");
    ctx.globalAlpha = 0.7;
    ctx.setLineDash([2, 4]);
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, plotH);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;
  }
}

function tracePath(
  ctx: CanvasRenderingContext2D,
  db: Float32Array,
  view: SpectrumView,
  width: number,
  height: number,
  dbWindow: DbWindow,
): void {
  const n = db.length;
  const first = view.start * (n - 1);
  const last = view.end * (n - 1);
  ctx.beginPath();
  for (let x = 0; x < width; x++) {
    const from = first + ((last - first) * x) / width;
    const to = first + ((last - first) * (x + 1)) / width;
    const lo = Math.max(0, Math.floor(from));
    const hi = Math.min(n - 1, Math.max(lo, Math.floor(to)));
    let peak = Number.NEGATIVE_INFINITY;
    for (let i = lo; i <= hi; i++) {
      const value = db[i] ?? Number.NEGATIVE_INFINITY;
      if (value > peak) {
        peak = value;
      }
    }
    const y = (1 - traceUnit(peak, dbWindow)) * height;
    if (x === 0) {
      ctx.moveTo(x, y);
    } else {
      ctx.lineTo(x, y);
    }
  }
}

function formatTick(hz: number, visibleHz: number): string {
  const decimals = visibleHz >= 5e6 ? 1 : visibleHz >= 5e5 ? 2 : visibleHz >= 5e4 ? 3 : 4;
  return (hz / 1e6).toFixed(decimals);
}
