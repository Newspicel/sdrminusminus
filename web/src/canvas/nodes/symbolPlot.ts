import type { Trend } from "../../components/baseband";
import { token } from "../../lib/tokens";

const PAD = { left: 34, right: 6, top: 8, bottom: 16 };

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
): void {
  const ctx = fit(canvas);
  if (ctx === null || bins.length === 0) {
    return;
  }
  const width = canvas.width;
  const height = canvas.height;
  const plotH = height - PAD.bottom;
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
    const barH = value * (plotH - PAD.top);
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
    ctx.moveTo(x, PAD.top);
    ctx.lineTo(x, plotH);
    ctx.stroke();
    ctx.fillText(level.toFixed(level % 1 === 0 ? 0 : 2), x, height - 4);
  }
  ctx.setLineDash([]);
}

export function drawTrend(
  canvas: HTMLCanvasElement,
  series: readonly TrendSeries[],
  unit: string,
  zero: boolean,
): void {
  const ctx = fit(canvas);
  if (ctx === null) {
    return;
  }
  const width = canvas.width;
  const height = canvas.height;
  const plotW = width - PAD.left - PAD.right;
  const plotH = height - PAD.top - PAD.bottom;
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

  const toY = (value: number): number => PAD.top + (1 - (value - min) / (max - min)) * plotH;

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
    ctx.fillText(label, at, height - 4);
    at += ctx.measureText(label).width + 10;
  }
  ctx.fillStyle = token("plot-ink-dim");
  ctx.textAlign = "right";
  ctx.fillText(unit, width - PAD.right, height - 4);
}
