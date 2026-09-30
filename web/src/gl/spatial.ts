import { recordEvent } from "../lib/diagnostics";
import type { SpatialSpectrumFrame } from "../lib/frame";
import { backingPx, pixelRatio, zoomOf } from "./raster";

export const TRAIL_ROWS = 160;

const SATURATION = 0.85;

export interface TrailView {
  push(frame: SpatialSpectrumFrame, offsetDeg: number): void;
  clear(): void;
  dispose(): void;
}

export function bearingHue(deg: number): number {
  return ((deg % 360) + 360) % 360;
}

export function columnArgmax(
  cells: Uint8Array,
  bearings: number,
  bins: number,
  col: number,
): { row: number; level: number } {
  let best = 0;
  let level = -1;
  for (let row = 0; row < bearings; row++) {
    const value = cells[row * bins + col] ?? 0;
    if (value > level) {
      level = value;
      best = row;
    }
  }
  return { row: best, level: Math.max(0, level) };
}

export function hueRgb(hueDeg: number, value: number): [number, number, number] {
  const h = bearingHue(hueDeg) / 60;
  const v = Math.min(1, Math.max(0, value));
  const chroma = v * SATURATION;
  const second = chroma * (1 - Math.abs((h % 2) - 1));
  const base = v - chroma;
  const sector = Math.floor(h) % 6;
  const [r, g, b] =
    sector === 0
      ? [chroma, second, 0]
      : sector === 1
        ? [second, chroma, 0]
        : sector === 2
          ? [0, chroma, second]
          : sector === 3
            ? [0, second, chroma]
            : sector === 4
              ? [second, 0, chroma]
              : [chroma, 0, second];
  return [Math.round((r + base) * 255), Math.round((g + base) * 255), Math.round((b + base) * 255)];
}

export function trailRow(
  frame: SpatialSpectrumFrame,
  offsetDeg: number,
  out: Uint8ClampedArray,
  at = 0,
): Uint8ClampedArray {
  const { bearings, bins, cells } = frame;
  for (let col = 0; col < bins; col++) {
    const peak = columnArgmax(cells, bearings, bins, col);
    const [r, g, b] = hueRgb(
      (peak.row * 360) / Math.max(1, bearings) + offsetDeg,
      peak.level / 255,
    );
    const index = at + col * 4;
    out[index] = r;
    out[index + 1] = g;
    out[index + 2] = b;
    out[index + 3] = 255;
  }
  return out;
}

export function attachTrail(canvas: HTMLCanvasElement, rows = TRAIL_ROWS): TrailView {
  const context = canvas.getContext("2d");
  if (context === null) {
    recordEvent("error", "spatial", "no 2d canvas");
  }
  let scratch: HTMLCanvasElement | null = null;
  let image: ImageData | null = null;

  const blit = (): void => {
    if (context === null || scratch === null || image === null) {
      return;
    }
    const ratio = pixelRatio(
      window.devicePixelRatio,
      zoomOf(canvas.clientWidth, canvas.offsetWidth || canvas.clientWidth),
    );
    const width = backingPx(canvas.clientWidth, ratio);
    const height = backingPx(canvas.clientHeight, ratio);
    if (width === 0 || height === 0) {
      return;
    }
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    scratch.getContext("2d")?.putImageData(image, 0, 0);
    context.imageSmoothingEnabled = false;
    context.clearRect(0, 0, width, height);
    context.drawImage(scratch, 0, 0, width, height);
  };

  return {
    push(frame, offsetDeg) {
      if (frame.bins === 0 || frame.bearings === 0) {
        return;
      }
      scratch ??= document.createElement("canvas");
      if (image === null || image.width !== frame.bins) {
        scratch.width = frame.bins;
        scratch.height = rows;
        image = new ImageData(frame.bins, rows);
      }
      const stride = frame.bins * 4;
      image.data.copyWithin(stride, 0, stride * (rows - 1));
      trailRow(frame, offsetDeg, image.data, 0);
      blit();
    },
    clear() {
      image?.data.fill(0);
      blit();
    },
    dispose() {
      scratch = null;
      image = null;
    },
  };
}
