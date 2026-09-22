import type { DbWindow } from "./spectrumTraces";

export const AVERAGE_CHOICES = [1, 2, 4, 8, 16] as const;
export type AverageFrames = (typeof AVERAGE_CHOICES)[number];

export class VideoAverage {
  private power = new Float32Array(0);
  private out = new Float32Array(0);
  private primed = false;

  reset(): void {
    this.primed = false;
  }

  apply(db: Float32Array, frames: number): Float32Array {
    if (frames <= 1) {
      this.primed = false;
      return db;
    }
    if (db.length !== this.power.length) {
      this.power = new Float32Array(db.length);
      this.out = new Float32Array(db.length);
      this.primed = false;
    }
    const weight = this.primed ? 1 / frames : 1;
    for (let i = 0; i < db.length; i++) {
      const level = 10 ** ((db[i] ?? 0) / 10);
      const held = (this.power[i] ?? 0) + (level - (this.power[i] ?? 0)) * weight;
      this.power[i] = held;
      this.out[i] = 10 * Math.log10(held + 1e-30);
    }
    this.primed = true;
    return this.out;
  }
}

export function quantizeDb(db: Float32Array, window: DbWindow, out: Uint8Array | null): Uint8Array {
  const dst = out !== null && out.length === db.length ? out : new Uint8Array(db.length);
  const span = window.max - window.min;
  if (!(span > 0)) {
    dst.fill(0);
    return dst;
  }
  const scale = 255 / span;
  for (let i = 0; i < db.length; i++) {
    dst[i] = Math.min(255, Math.max(0, Math.round(((db[i] ?? 0) - window.min) * scale)));
  }
  return dst;
}
