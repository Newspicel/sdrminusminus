import { type Colormap, DEFAULT_COLORMAP, sampleColormap } from "./colormap";
import { backingPx, pixelRatio, zoomOf } from "./raster";

export { COLORMAPS, type Colormap, DEFAULT_COLORMAP } from "./colormap";

export interface SurfaceFrame {
  ranges: number;
  dopplers: number;
  cells: Uint8Array;
}

export interface SurfaceMark {
  range: number;
  doppler: number;
}

export interface SurfaceView {
  draw(frame: SurfaceFrame, marks?: readonly SurfaceMark[]): void;
  setColormap(name: Colormap): void;
  dispose(): void;
}

const PALETTE_STEPS = 256;
const MARK_RADIUS_PX = 5;

function palette(map: Colormap): Uint8ClampedArray {
  const table = new Uint8ClampedArray(PALETTE_STEPS * 3);
  for (let step = 0; step < PALETTE_STEPS; step++) {
    const [r, g, b] = sampleColormap(map, step / (PALETTE_STEPS - 1));
    table[step * 3] = r * 255;
    table[step * 3 + 1] = g * 255;
    table[step * 3 + 2] = b * 255;
  }
  return table;
}

/// Paints a range–Doppler surface: range across, Doppler up, the most negative shift at the
/// bottom so a target closing and a target opening lean opposite ways.
///
/// A surface is one small image a few times a second, so it is coloured into an `ImageData` and
/// blitted rather than uploaded to the GPU — the waterfall's machinery buys nothing at this size.
export function attachSurface(canvas: HTMLCanvasElement): SurfaceView {
  const context = canvas.getContext("2d");
  let map = DEFAULT_COLORMAP;
  let table = palette(map);
  let image: ImageData | null = null;
  let scratch: HTMLCanvasElement | null = null;

  const colourInto = (frame: SurfaceFrame): HTMLCanvasElement | null => {
    const { ranges, dopplers, cells } = frame;
    if (ranges === 0 || dopplers === 0 || cells.length < ranges * dopplers) {
      return null;
    }
    if (scratch === null) {
      scratch = document.createElement("canvas");
    }
    if (scratch.width !== ranges || scratch.height !== dopplers) {
      scratch.width = ranges;
      scratch.height = dopplers;
      image = null;
    }
    const paint = scratch.getContext("2d");
    if (paint === null) {
      return null;
    }
    if (image === null || image.width !== ranges || image.height !== dopplers) {
      image = paint.createImageData(ranges, dopplers);
    }
    const pixels = image.data;
    for (let row = 0; row < dopplers; row++) {
      const source = row * ranges;
      const target = (dopplers - 1 - row) * ranges * 4;
      for (let column = 0; column < ranges; column++) {
        const level = cells[source + column] ?? 0;
        const at = target + column * 4;
        pixels[at] = table[level * 3] ?? 0;
        pixels[at + 1] = table[level * 3 + 1] ?? 0;
        pixels[at + 2] = table[level * 3 + 2] ?? 0;
        pixels[at + 3] = 255;
      }
    }
    paint.putImageData(image, 0, 0);
    return scratch;
  };

  return {
    draw(frame, marks = []) {
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
      const painted = colourInto(frame);
      context.imageSmoothingEnabled = false;
      context.clearRect(0, 0, width, height);
      if (painted === null) {
        return;
      }
      context.drawImage(painted, 0, 0, width, height);
      if (marks.length === 0) {
        return;
      }
      context.strokeStyle = "#ffffff";
      context.lineWidth = Math.max(1, ratio);
      for (const mark of marks) {
        const x = ((mark.range + 0.5) / frame.ranges) * width;
        const y = ((frame.dopplers - 0.5 - mark.doppler) / frame.dopplers) * height;
        context.beginPath();
        context.arc(x, y, MARK_RADIUS_PX * ratio, 0, Math.PI * 2);
        context.stroke();
      }
    },
    setColormap(name) {
      if (name === map) {
        return;
      }
      map = name;
      table = palette(map);
    },
    dispose() {
      scratch = null;
      image = null;
    },
  };
}
