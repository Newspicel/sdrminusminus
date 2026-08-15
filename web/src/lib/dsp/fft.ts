export function fft(re: Float32Array, im: Float32Array): void {
  const n = re.length;
  if (n !== im.length || n < 2 || (n & (n - 1)) !== 0) {
    throw new Error(`fft needs a power-of-two length, got ${n} and ${im.length}`);
  }

  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) {
      j ^= bit;
    }
    j ^= bit;
    if (i < j) {
      const tr = re[i] ?? 0;
      const ti = im[i] ?? 0;
      re[i] = re[j] ?? 0;
      im[i] = im[j] ?? 0;
      re[j] = tr;
      im[j] = ti;
    }
  }

  for (let len = 2; len <= n; len <<= 1) {
    const step = (-2 * Math.PI) / len;
    const half = len >> 1;
    for (let start = 0; start < n; start += len) {
      for (let k = 0; k < half; k++) {
        const angle = step * k;
        const wr = Math.cos(angle);
        const wi = Math.sin(angle);
        const a = start + k;
        const b = a + half;
        const xr = re[b] ?? 0;
        const xi = im[b] ?? 0;
        const tr = xr * wr - xi * wi;
        const ti = xr * wi + xi * wr;
        re[b] = (re[a] ?? 0) - tr;
        im[b] = (im[a] ?? 0) - ti;
        re[a] = (re[a] ?? 0) + tr;
        im[a] = (im[a] ?? 0) + ti;
      }
    }
  }
}

export function hann(n: number): Float32Array {
  const window = new Float32Array(n);
  if (n === 0) {
    return window;
  }
  if (n === 1) {
    window[0] = 1;
    return window;
  }
  for (let i = 0; i < n; i++) {
    window[i] = 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / n);
  }
  return window;
}

export function coherentGain(window: Float32Array): number {
  let sum = 0;
  for (const value of window) {
    sum += value;
  }
  return sum;
}

export class SpectrumAnalyzer {
  readonly size: number;
  private readonly re: Float32Array;
  private readonly im: Float32Array;
  private readonly window: Float32Array;
  private readonly invGain: number;

  constructor(size: number) {
    this.size = size;
    this.re = new Float32Array(size);
    this.im = new Float32Array(size);
    this.window = hann(size);
    this.invGain = 1 / Math.max(coherentGain(this.window), Number.MIN_VALUE);
  }

  powerDb(samples: Float32Array, out: Float32Array): Float32Array {
    const n = this.size;
    const db = out.length === n ? out : new Float32Array(n);
    const available = Math.min(n, samples.length >> 1);
    for (let i = 0; i < n; i++) {
      const w = i < available ? (this.window[i] ?? 0) : 0;
      this.re[i] = (samples[i * 2] ?? 0) * w;
      this.im[i] = (samples[i * 2 + 1] ?? 0) * w;
    }
    fft(this.re, this.im);

    const half = n >> 1;
    for (let raw = 0; raw < n; raw++) {
      const shifted = (raw + half) % n;
      const rr = this.re[raw] ?? 0;
      const ii = this.im[raw] ?? 0;
      const mag = Math.sqrt(rr * rr + ii * ii) * this.invGain;
      db[shifted] = 20 * Math.log10(mag + 1e-12);
    }
    return db;
  }
}
