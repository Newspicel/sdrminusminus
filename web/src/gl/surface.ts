import { recordEvent } from "../lib/diagnostics";
import { type Colormap, DEFAULT_COLORMAP, sampleColormap } from "./colormap";
import { backingPx, pixelRatio, zoomOf } from "./raster";

export { COLORMAPS, type Colormap, DEFAULT_COLORMAP } from "./colormap";

export interface GridFrame {
  cols: number;
  rows: number;
  cells: Uint8Array;
}

export interface GridOptions {
  flipY: boolean;
  transparentFloor: boolean;
}

export interface GridView {
  draw(frame: GridFrame): void;
  setColormap(name: Colormap): void;
  dispose(): void;
}

export interface GridCell {
  col: number;
  row: number;
}

export const PALETTE_STEPS = 256;

const FLOOR_OPACITY = 0.85;
const FLOOR_GAMMA = 0.7;

export function paletteTable(map: Colormap): Uint8ClampedArray {
  const table = new Uint8ClampedArray(PALETTE_STEPS * 3);
  for (let step = 0; step < PALETTE_STEPS; step++) {
    const [r, g, b] = sampleColormap(map, step / (PALETTE_STEPS - 1));
    table[step * 3] = r * 255;
    table[step * 3 + 1] = g * 255;
    table[step * 3 + 2] = b * 255;
  }
  return table;
}

export function floorAlpha(level: number): number {
  return level === 0 ? 0 : Math.round(255 * FLOOR_OPACITY * (level / 255) ** FLOOR_GAMMA);
}

export function gridPixels(
  frame: GridFrame,
  table: Uint8ClampedArray,
  options: GridOptions,
  out?: Uint8ClampedArray,
): Uint8ClampedArray {
  const { cols, rows, cells } = frame;
  const length = cols * rows * 4;
  const pixels = out !== undefined && out.length === length ? out : new Uint8ClampedArray(length);
  for (let row = 0; row < rows; row++) {
    const source = (options.flipY ? rows - 1 - row : row) * cols;
    const target = row * cols * 4;
    for (let col = 0; col < cols; col++) {
      const level = cells[source + col] ?? 0;
      const at = target + col * 4;
      pixels[at] = table[level * 3] ?? 0;
      pixels[at + 1] = table[level * 3 + 1] ?? 0;
      pixels[at + 2] = table[level * 3 + 2] ?? 0;
      pixels[at + 3] = options.transparentFloor ? floorAlpha(level) : 255;
    }
  }
  return pixels;
}

export function gridCellAt(
  x: number,
  y: number,
  box: { w: number; h: number },
  frame: { cols: number; rows: number },
  flipY: boolean,
): GridCell | null {
  if (box.w <= 0 || box.h <= 0 || frame.cols <= 0 || frame.rows <= 0) {
    return null;
  }
  if (x < 0 || y < 0 || x >= box.w || y >= box.h) {
    return null;
  }
  const col = Math.min(frame.cols - 1, Math.floor((x / box.w) * frame.cols));
  const shown = Math.min(frame.rows - 1, Math.floor((y / box.h) * frame.rows));
  return { col, row: flipY ? frame.rows - 1 - shown : shown };
}

export function validGrid(frame: GridFrame): boolean {
  return frame.cols > 0 && frame.rows > 0 && frame.cells.length >= frame.cols * frame.rows;
}

export function attachGrid(canvas: HTMLCanvasElement, options: GridOptions): GridView {
  const context = canvas.getContext("2d");
  if (context === null) {
    recordEvent("error", "grid", "no 2d canvas");
  }
  let table = paletteTable(DEFAULT_COLORMAP);
  let map: Colormap = DEFAULT_COLORMAP;
  let scratch: HTMLCanvasElement | null = null;
  let image: ImageData | null = null;

  const paint = (frame: GridFrame): HTMLCanvasElement | null => {
    scratch ??= document.createElement("canvas");
    if (scratch.width !== frame.cols || scratch.height !== frame.rows) {
      scratch.width = frame.cols;
      scratch.height = frame.rows;
    }
    const target = scratch.getContext("2d");
    if (target === null) {
      return null;
    }
    if (image === null || image.width !== frame.cols || image.height !== frame.rows) {
      image = target.createImageData(frame.cols, frame.rows);
    }
    gridPixels(frame, table, options, image.data);
    target.putImageData(image, 0, 0);
    return scratch;
  };

  return {
    draw(frame) {
      if (context === null) {
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
      context.clearRect(0, 0, width, height);
      if (!validGrid(frame)) {
        return;
      }
      const painted = paint(frame);
      if (painted === null) {
        return;
      }
      context.imageSmoothingEnabled = false;
      context.drawImage(painted, 0, 0, width, height);
    },
    setColormap(name) {
      if (name !== map) {
        map = name;
        table = paletteTable(name);
      }
    },
    dispose() {
      scratch = null;
      image = null;
    },
  };
}
