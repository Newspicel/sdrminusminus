import type { SymbolState, Trend } from "../../components/baseband";
import { token } from "../../lib/tokens";
import { PLOT_FONT, type PreparedCanvas, prepareCanvas } from "./scopePlot";

const PAD = { left: 40, right: 8, top: 6, bottom: 18 };
const STATE_LABELS = 78;
const STATE_READOUT = 132;
const STATE_EXTENT = 1.1;
const MINOR_TICKS = 5;

export interface PlotInset {
  top: number;
  bottom: number;
}

export const NO_INSET: PlotInset = { top: 0, bottom: 0 };

export interface TrendSeries {
  trend: Trend;
  colour: string;
  label: string;
}

function fit(canvas: HTMLCanvasElement): PreparedCanvas | null {
  const prepared = prepareCanvas(canvas);
  if (prepared !== null) {
    prepared.ctx.font = PLOT_FONT;
    prepared.ctx.lineWidth = 1;
  }
  return prepared;
}

export interface PlotBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function drawGraticule(
  ctx: CanvasRenderingContext2D,
  box: PlotBox,
  columns: number,
  rows: number,
): void {
  const x0 = Math.round(box.x) + 0.5;
  const y0 = Math.round(box.y) + 0.5;
  const x1 = Math.round(box.x + box.w) - 0.5;
  const y1 = Math.round(box.y + box.h) - 0.5;
  ctx.strokeStyle = token("plot-grid");
  ctx.globalAlpha = 0.55;
  ctx.beginPath();
  for (let i = 1; i < columns; i++) {
    const x = Math.round(box.x + (box.w * i) / columns) + 0.5;
    ctx.moveTo(x, y0);
    ctx.lineTo(x, y1);
  }
  for (let i = 1; i < rows; i++) {
    const y = Math.round(box.y + (box.h * i) / rows) + 0.5;
    ctx.moveTo(x0, y);
    ctx.lineTo(x1, y);
  }
  ctx.stroke();
  ctx.globalAlpha = 1;
  ctx.strokeRect(x0, y0, x1 - x0, y1 - y0);

  const cx = Math.round(box.x + box.w / 2) + 0.5;
  const cy = Math.round(box.y + box.h / 2) + 0.5;
  ctx.strokeStyle = token("plot-ink-dim");
  ctx.globalAlpha = 0.45;
  ctx.beginPath();
  for (let i = 1; i < columns * MINOR_TICKS; i++) {
    const x = Math.round(box.x + (box.w * i) / (columns * MINOR_TICKS)) + 0.5;
    const tick = i % MINOR_TICKS === 0 ? 4 : 2;
    ctx.moveTo(x, cy - tick);
    ctx.lineTo(x, cy + tick);
  }
  for (let i = 1; i < rows * MINOR_TICKS; i++) {
    const y = Math.round(box.y + (box.h * i) / (rows * MINOR_TICKS)) + 0.5;
    const tick = i % MINOR_TICKS === 0 ? 4 : 2;
    ctx.moveTo(cx - tick, y);
    ctx.lineTo(cx + tick, y);
  }
  ctx.stroke();
  ctx.globalAlpha = 1;
}

export function plotLabel(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  align: CanvasTextAlign = "left",
): void {
  const width = ctx.measureText(text).width;
  const left = align === "right" ? x - width : align === "center" ? x - width / 2 : x;
  ctx.fillStyle = token("plot-bg");
  ctx.globalAlpha = 0.75;
  ctx.fillRect(left - 2, y - 9, width + 4, 12);
  ctx.globalAlpha = 1;
  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = align;
  ctx.fillText(text, x, y);
}

export function drawHistogram(
  canvas: HTMLCanvasElement,
  bins: Float32Array,
  reference: readonly number[],
  scale: number,
  inset: PlotInset = NO_INSET,
): void {
  const prepared = fit(canvas);
  if (prepared === null || bins.length === 0) {
    return;
  }
  const { ctx, width, height } = prepared;
  const box = {
    x: PAD.right,
    y: PAD.top + inset.top,
    w: width - 2 * PAD.right,
    h: height - PAD.top - PAD.bottom - inset.top - inset.bottom,
  };
  if (box.w <= 0 || box.h <= 0) {
    return;
  }
  drawGraticule(ctx, box, 8, 4);
  const span = scale > 0 ? scale : 1;
  const toX = (value: number): number => box.x + ((value / span + 1) / 2) * box.w;
  const foot = box.y + box.h;

  ctx.fillStyle = token("plot-trace");
  ctx.globalAlpha = 0.85;
  const step = box.w / bins.length;
  for (let i = 0; i < bins.length; i++) {
    const value = bins[i] ?? 0;
    if (value <= 0) {
      continue;
    }
    const barH = value * box.h * 0.92;
    ctx.fillRect(box.x + i * step, foot - barH, Math.max(1, step - 0.5), barH);
  }
  ctx.globalAlpha = 1;

  ctx.strokeStyle = token("plot-hold");
  ctx.globalAlpha = 0.7;
  ctx.setLineDash([3, 3]);
  for (const level of reference) {
    const x = Math.round(toX(level)) + 0.5;
    if (x < box.x || x > box.x + box.w) {
      continue;
    }
    ctx.beginPath();
    ctx.moveTo(x, box.y);
    ctx.lineTo(x, foot);
    ctx.stroke();
    plotLabel(ctx, level.toFixed(level % 1 === 0 ? 0 : 2), x, foot + 11, "center");
  }
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;
  plotLabel(ctx, "share", box.x + 4, box.y + 11);
  plotLabel(ctx, "level", box.x + box.w - 4, box.y + 11, "right");
}

export function drawTrend(
  canvas: HTMLCanvasElement,
  series: readonly TrendSeries[],
  unit: string,
  zero: boolean,
  inset: PlotInset = NO_INSET,
): void {
  const prepared = fit(canvas);
  if (prepared === null) {
    return;
  }
  const { ctx, width, height } = prepared;
  const box = {
    x: PAD.left,
    y: PAD.top + inset.top,
    w: width - PAD.left - PAD.right,
    h: height - PAD.top - PAD.bottom - inset.top - inset.bottom,
  };
  if (box.w <= 0 || box.h <= 0) {
    return;
  }
  drawGraticule(ctx, box, 10, 4);

  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;
  let longest = 0;
  for (const { trend } of series) {
    if (trend.length === 0) {
      continue;
    }
    const range = trend.range();
    min = Math.min(min, range.min);
    max = Math.max(max, range.max);
    longest = Math.max(longest, trend.length);
  }
  if (longest === 0) {
    return;
  }
  if (zero) {
    const reach = Math.max(Math.abs(min), Math.abs(max), 1);
    min = -reach;
    max = reach;
  }
  const pad = Math.max((max - min) * 0.1, 0.5);
  min -= pad;
  max += pad;
  const toY = (value: number): number => box.y + (1 - (value - min) / (max - min)) * box.h;

  const decimals = Math.abs(max - min) < 10 ? 1 : 0;
  for (let i = 0; i <= 4; i++) {
    const value = min + ((max - min) * i) / 4;
    plotLabel(ctx, value.toFixed(decimals), box.x - 5, toY(value) + 3, "right");
  }

  for (const { trend, colour } of series) {
    if (trend.length < 2) {
      continue;
    }
    ctx.strokeStyle = colour;
    ctx.lineWidth = 1.5;
    ctx.lineJoin = "round";
    ctx.beginPath();
    for (let i = 0; i < trend.length; i++) {
      const x = box.x + (i / (longest - 1)) * box.w;
      const y = toY(trend.sample(i));
      if (i === 0) {
        ctx.moveTo(x, y);
      } else {
        ctx.lineTo(x, y);
      }
    }
    ctx.stroke();
  }
  ctx.lineWidth = 1;

  let at = box.x + 6;
  for (const { colour, label } of series) {
    ctx.fillStyle = colour;
    ctx.fillRect(at, box.y + 7, 8, 2);
    plotLabel(ctx, label, at + 12, box.y + 11);
    at += ctx.measureText(label).width + 26;
  }
  plotLabel(ctx, unit, box.x + box.w - 6, box.y + 11, "right");
  plotLabel(ctx, "older", box.x, box.y + box.h + 12);
  plotLabel(ctx, "now", box.x + box.w, box.y + box.h + 12, "right");
}

export function drawStates(
  canvas: HTMLCanvasElement,
  states: readonly SymbolState[],
  signed: boolean,
  inset: PlotInset = NO_INSET,
): void {
  const prepared = fit(canvas);
  if (prepared === null || states.length === 0) {
    return;
  }
  const { ctx, width, height } = prepared;
  const top = PAD.top + inset.top;
  const foot = height - PAD.bottom - inset.bottom;
  const left = STATE_LABELS;
  const right = width - STATE_READOUT;
  const plotH = foot - top;
  if (right - left < 40 || plotH < states.length * 4) {
    return;
  }
  const toX = (error: number): number => {
    const unit = signed ? (error / STATE_EXTENT + 1) / 2 : error / STATE_EXTENT;
    return left + Math.min(1, Math.max(0, unit)) * (right - left);
  };

  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = "center";
  for (const mark of signed ? [-1, 0, 1] : [0, 1]) {
    const x = Math.round(toX(mark)) + 0.5;
    ctx.strokeStyle = mark === 0 ? token("plot-ink-dim") : token("plot-grid");
    ctx.setLineDash(mark === 0 ? [] : [2, 3]);
    ctx.beginPath();
    ctx.moveTo(x, top);
    ctx.lineTo(x, foot);
    ctx.stroke();
    ctx.fillText(mark === 0 ? "ideal" : "slice", x, foot + 12);
  }
  ctx.setLineDash([]);
  ctx.textAlign = "left";
  ctx.fillText("share offset spread", right + 8, foot + 12);

  const rowH = plotH / states.length;
  const boxH = Math.min(rowH * 0.52, 18);
  for (const [row, state] of states.entries()) {
    const mid = top + rowH * (row + 0.5);
    ctx.fillStyle = token("plot-ink-dim");
    ctx.textAlign = "left";
    ctx.fillText(state.bits, 4, mid + 3);
    ctx.textAlign = "right";
    ctx.fillText(ideal(state, signed), left - 8, mid + 3);

    ctx.strokeStyle = token("plot-grid");
    ctx.beginPath();
    ctx.moveTo(left, Math.round(mid) + 0.5);
    ctx.lineTo(right, Math.round(mid) + 0.5);
    ctx.stroke();

    ctx.textAlign = "left";
    if (state.count === 0) {
      ctx.fillText("never decided", right + 8, mid + 3);
      continue;
    }

    const meanX = toX(state.mean);
    if (Number.isFinite(state.sigma)) {
      const lo = toX(state.mean - state.sigma);
      const hi = toX(state.mean + state.sigma);
      ctx.fillStyle = token("plot-trace");
      ctx.globalAlpha = 0.25;
      ctx.fillRect(lo, mid - boxH / 2, Math.max(1, hi - lo), boxH);
      ctx.globalAlpha = 1;
    }

    ctx.strokeStyle = token("plot-hold");
    ctx.globalAlpha = 0.65;
    const peakX = Math.round(toX(state.peak)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(meanX, mid);
    ctx.lineTo(peakX, mid);
    ctx.moveTo(peakX, mid - boxH / 3);
    ctx.lineTo(peakX, mid + boxH / 3);
    ctx.stroke();
    ctx.globalAlpha = 1;

    ctx.strokeStyle = token("plot-trace");
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(Math.round(meanX) + 0.5, mid - boxH / 2);
    ctx.lineTo(Math.round(meanX) + 0.5, mid + boxH / 2);
    ctx.stroke();
    ctx.lineWidth = 1;

    if (rowH >= 12) {
      ctx.fillStyle = token("plot-ink-dim");
      ctx.fillText(numerics(state), right + 8, mid + 3);
    }
  }
}

function ideal(state: SymbolState, signed: boolean): string {
  return signed ? formatLevel(state.i) : `${formatLevel(state.i)},${formatLevel(state.q)}`;
}

function formatLevel(value: number): string {
  return value.toFixed(Number.isInteger(value) ? 0 : 2);
}

function numerics(state: SymbolState): string {
  return `${(state.share * 100).toFixed(0).padStart(2)}%  ${percent(state.mean, true)}  ${percent(state.sigma, false)}`;
}

function percent(value: number, sign: boolean): string {
  if (!Number.isFinite(value)) {
    return "  – ";
  }
  const shown = Math.round(value * 100);
  return `${sign && shown >= 0 ? "+" : ""}${shown}%`;
}
