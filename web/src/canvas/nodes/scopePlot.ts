// The scope's trace panel: the phosphor layer, the grid, both axes and every trace drawn over
// them. Split out of `ScopeFace` so that file stays about the gestures and this one about paint.

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
  spanToView,
  viewWidth,
} from "../../components/spectrumView";
import type { Colormap } from "../../gl/colormap";
import { pixelRatio, zoomOf } from "../../gl/raster";
import { token } from "../../lib/tokens";

/** Rows the frequency axis reserves at the bottom of the trace canvas, in CSS pixels. */
export const AXIS_H = 16;
/** The translucency SDR++ draws its fill under the trace at (`ImGuiCol_PlotLines` at 0.2). */
const TRACE_FILL_ALPHA = 0.2;

/**
 * Ink for each accumulated trace.
 *
 * Only two hues are licensed for data on the plot (see `index.css`), and both are spent: cyan on
 * the live trace, yellow on the peak. The average and the floor are derived from the same
 * measurement rather than being a third and fourth kind of it, so they are drawn achromatically —
 * which also keeps them legible on top of the phosphor layer, where a hue would collide with the
 * colormap.
 */
const TRACE_INK: Record<TraceMode, string> = {
  peak: "plot-hold",
  average: "plot-ink",
  min: "plot-ink-dim",
};

export interface PlotTrace {
  mode: TraceMode;
  /** Levels in dBFS, one per bin of the frame being drawn. */
  db: Float32Array;
}

export interface PlotFrame {
  centerHz: number;
  spanHz: number;
  /** Levels in dBFS, one per bin. */
  db: Float32Array;
}

export interface PlotOptions {
  frame: PlotFrame | null;
  view: SpectrumView;
  /** The dB range mapped onto the plot's height. Every trace and the phosphor share it, so the
   * curves stay comparable even while the server's own window moves under them. */
  window: DbWindow;
  traces: readonly PlotTrace[];
  density: DensityLayer | null;
}

/**
 * An offscreen bitmap the size of a density grid, recoloured only when the grid or the ramp has
 * actually changed and then scaled into whatever rectangle the plot gives it.
 *
 * The scaling is the reason it exists at all: `putImageData` ignores the destination rectangle,
 * and every grid here is deliberately coarser than the panel it is drawn into. Held across frames
 * because building a hundred-thousand-pixel `ImageData` sixty times a second is the whole cost.
 */
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

  /** Mark the bitmap stale; the next `blit` recolours it. */
  invalidate(): void {
    this.dirty = true;
  }

  /** Draw into `ctx`, recolouring through `recolour` first if anything has changed since the
   * last call. */
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
    // Smoothed on the way up: nearest neighbour would show the grid's cells as blocks rather
    // than as a glow.
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(this.canvas, box.x, box.y, box.w, box.h);
  }
}

/** The persistence display's grid, its bitmap and the ramp it is coloured with. Held by the
 * caller across frames — the whole point of the layer is what it remembers. */
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

  /** Fold one frame in. Called per arriving frame and not per animation frame: the decay is a
   * property of the signal's rate, so a plot that stops receiving must stop fading too. */
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

/** Size `canvas` to its CSS box at the right device ratio and return a context in CSS units. */
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

/** The trace panel, bottom layer first. Drawn from the frame's own metadata, so the numbers on
 * screen are the ones the server measured. */
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
  // A fill under the trace gives the noise floor a body to read against without spending a
  // second colour on it.
  ctx.lineTo(width, plotH);
  ctx.lineTo(0, plotH);
  ctx.closePath();
  ctx.fillStyle = token("plot-trace");
  ctx.globalAlpha = TRACE_FILL_ALPHA;
  ctx.fill();
  ctx.globalAlpha = 1;
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
  // The grid is opaque: `plot-grid` is already the near-black SDR++ draws its scale lines in, so
  // fading it further would leave nothing on the ground. Half-pixel offsets so a 1px rule lands
  // on one device row instead of straddling two.
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

/** One screen column per pixel, taking the maximum of every bin that falls in it: decimating by
 * sampling would drop exactly the narrow carriers the display exists to show. */
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
    // `floor(to)` and not `ceil(to) - 1`: the two agree except where a column ends exactly on a
    // bin, and there the latter drops it — which is every last column of an unzoomed plot.
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

/** Tick labels carry only the digits the current zoom can distinguish — six decimals on a 2 MHz
 * span is noise the reader has to parse past. */
function formatTick(hz: number, visibleHz: number): string {
  const decimals = visibleHz >= 5e6 ? 1 : visibleHz >= 5e5 ? 2 : visibleHz >= 5e4 ? 3 : 4;
  return (hz / 1e6).toFixed(decimals);
}
