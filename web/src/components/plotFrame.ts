import { COLORMAPS, type Colormap, sampleColormap } from "../gl/colormap";
import { niceStep, niceTicks, tickLabel } from "../lib/ticks";
import type { Options } from "./controls";

export interface Gutters {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface PlotRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Tick {
  px: number;
  label: string;
}

const GRADIENT_STOPS = 12;

export function plotRect(width: number, height: number, gutters: Gutters): PlotRect {
  return {
    x: gutters.left,
    y: gutters.top,
    w: Math.max(0, width - gutters.left - gutters.right),
    h: Math.max(0, height - gutters.top - gutters.bottom),
  };
}

export function linearTicks(
  lo: number,
  hi: number,
  target: number,
  toPx: (value: number) => number,
): Tick[] {
  const step = niceStep(Math.abs(hi - lo), target);
  if (step === 0) {
    return [];
  }
  return niceTicks(lo, hi, target).map((value) => ({
    px: toPx(value),
    label: tickLabel(value, step),
  }));
}

export function colormapGradient(map: Colormap, direction: string): string {
  const stops = Array.from({ length: GRADIENT_STOPS }, (_, index) => {
    const [r, g, b] = sampleColormap(map, index / (GRADIENT_STOPS - 1));
    return `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;
  });
  return `linear-gradient(${direction}, ${stops.join(", ")})`;
}

export const COLOUR_OPTIONS: Options<Colormap> = COLORMAPS.map((name) => ({
  value: name,
  label: name.charAt(0).toUpperCase() + name.slice(1),
}));
