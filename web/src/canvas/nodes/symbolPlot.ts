import type { SymbolState, Trend } from "../../components/baseband";
import { token } from "../../lib/tokens";

const PAD = { left: 34, right: 6, top: 8, bottom: 16 };
const STATE_LABELS = 78;
const STATE_READOUT = 132;
const STATE_EXTENT = 1.1;

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

function fit(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0) {
    return null;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return null;
  }
  ctx.clearRect(0, 0, width, height);
  ctx.font = "10px ui-monospace, monospace";
  return ctx;
}

export function drawHistogram(
  canvas: HTMLCanvasElement,
  bins: Float32Array,
  reference: readonly number[],
  scale: number,
  inset: PlotInset = NO_INSET,
): void {
  const ctx = fit(canvas);
  if (ctx === null || bins.length === 0) {
    return;
  }
  const width = canvas.width;
  const height = canvas.height;
  const top = PAD.top + inset.top;
  const plotH = height - PAD.bottom - inset.bottom;
  if (plotH <= top) {
    return;
  }
  const span = scale > 0 ? scale : 1;
  const toX = (value: number): number => ((value / span + 1) / 2) * width;

  ctx.strokeStyle = token("plot-grid");
  ctx.beginPath();
  ctx.moveTo(0, plotH + 0.5);
  ctx.lineTo(width, plotH + 0.5);
  ctx.stroke();

  ctx.fillStyle = token("plot-trace");
  const step = width / bins.length;
  for (let i = 0; i < bins.length; i++) {
    const value = bins[i] ?? 0;
    if (value <= 0) {
      continue;
    }
    const barH = value * (plotH - top);
    ctx.fillRect(i * step, plotH - barH, Math.max(1, step - 1), barH);
  }

  ctx.strokeStyle = token("plot-ink-dim");
  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = "center";
  ctx.setLineDash([2, 3]);
  for (const level of reference) {
    const x = Math.round(toX(level)) + 0.5;
    if (x < 0 || x > width) {
      continue;
    }
    ctx.beginPath();
    ctx.moveTo(x, top);
    ctx.lineTo(x, plotH);
    ctx.stroke();
    ctx.fillText(level.toFixed(level % 1 === 0 ? 0 : 2), x, plotH + 12);
  }
  ctx.setLineDash([]);
}

export function drawTrend(
  canvas: HTMLCanvasElement,
  series: readonly TrendSeries[],
  unit: string,
  zero: boolean,
  inset: PlotInset = NO_INSET,
): void {
  const ctx = fit(canvas);
  if (ctx === null) {
    return;
  }
  const width = canvas.width;
  const height = canvas.height;
  const top = PAD.top + inset.top;
  const foot = height - PAD.bottom - inset.bottom;
  const plotW = width - PAD.left - PAD.right;
  const plotH = foot - top;
  if (plotW <= 0 || plotH <= 0) {
    return;
  }

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
  const pad = (max - min) * 0.1;
  min -= pad;
  max += pad;

  const toY = (value: number): number => top + (1 - (value - min) / (max - min)) * plotH;

  ctx.strokeStyle = token("plot-grid");
  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = "right";
  for (let i = 0; i <= 2; i++) {
    const value = min + ((max - min) * i) / 2;
    const y = Math.round(toY(value)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(PAD.left, y);
    ctx.lineTo(width - PAD.right, y);
    ctx.stroke();
    ctx.fillText(value.toFixed(Math.abs(max - min) < 10 ? 1 : 0), PAD.left - 4, y + 3);
  }

  if (zero) {
    ctx.strokeStyle = token("plot-ink-dim");
    ctx.globalAlpha = 0.6;
    const y = Math.round(toY(0)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(PAD.left, y);
    ctx.lineTo(width - PAD.right, y);
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  for (const { trend, colour } of series) {
    if (trend.length < 2) {
      continue;
    }
    ctx.strokeStyle = colour;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let i = 0; i < trend.length; i++) {
      const x = PAD.left + (i / (longest - 1)) * plotW;
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

  ctx.textAlign = "left";
  let at = PAD.left + 2;
  for (const { colour, label } of series) {
    ctx.fillStyle = colour;
    ctx.fillText(label, at, foot + 12);
    at += ctx.measureText(label).width + 10;
  }
  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = "right";
  ctx.fillText(unit, width - PAD.right, foot + 12);
}

export function drawStates(
  canvas: HTMLCanvasElement,
  states: readonly SymbolState[],
  signed: boolean,
  inset: PlotInset = NO_INSET,
): void {
  const ctx = fit(canvas);
  if (ctx === null || states.length === 0) {
    return;
  }
  const width = canvas.width;
  const height = canvas.height;
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
