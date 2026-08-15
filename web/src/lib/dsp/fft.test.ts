import { describe, expect, it } from "vitest";
import { coherentGain, fft, hann, SpectrumAnalyzer } from "./fft";

function tone(n: number, cyclesPerBlock: number, amp = 1): Float32Array {
  const out = new Float32Array(n * 2);
  for (let i = 0; i < n; i++) {
    const phase = (2 * Math.PI * cyclesPerBlock * i) / n;
    out[i * 2] = Math.cos(phase) * amp;
    out[i * 2 + 1] = Math.sin(phase) * amp;
  }
  return out;
}

describe("fft", () => {
  it("transforms a DC input into a single bin", () => {
    const re = Float32Array.from([1, 1, 1, 1]);
    const im = new Float32Array(4);
    fft(re, im);
    expect(Array.from(re)).toEqual([4, 0, 0, 0]);
    expect(Array.from(im)).toEqual([0, 0, 0, 0]);
  });

  it("puts a one-cycle complex tone in bin 1", () => {
    const n = 16;
    const re = new Float32Array(n);
    const im = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      re[i] = Math.cos((2 * Math.PI * i) / n);
      im[i] = Math.sin((2 * Math.PI * i) / n);
    }
    fft(re, im);
    const mags = Array.from(re, (r, i) => Math.hypot(r, im[i] ?? 0));
    expect(mags[1]).toBeCloseTo(n, 3);
    for (let i = 0; i < n; i++) {
      if (i !== 1) {
        expect(mags[i]).toBeLessThan(1e-3);
      }
    }
  });

  it("matches a direct transform on an arbitrary signal", () => {
    const n = 32;
    const re = new Float32Array(n);
    const im = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      re[i] = Math.sin(i * 1.7) + 0.3 * Math.cos(i * 0.4);
      im[i] = Math.cos(i * 2.3) - 0.2 * Math.sin(i * 1.1);
    }
    const wantRe = new Float32Array(n);
    const wantIm = new Float32Array(n);
    for (let k = 0; k < n; k++) {
      let sr = 0;
      let si = 0;
      for (let t = 0; t < n; t++) {
        const angle = (-2 * Math.PI * k * t) / n;
        const c = Math.cos(angle);
        const s = Math.sin(angle);
        sr += (re[t] ?? 0) * c - (im[t] ?? 0) * s;
        si += (re[t] ?? 0) * s + (im[t] ?? 0) * c;
      }
      wantRe[k] = sr;
      wantIm[k] = si;
    }

    fft(re, im);
    for (let k = 0; k < n; k++) {
      expect(re[k]).toBeCloseTo(wantRe[k] ?? 0, 3);
      expect(im[k]).toBeCloseTo(wantIm[k] ?? 0, 3);
    }
  });

  it("refuses a length it cannot transform", () => {
    expect(() => fft(new Float32Array(6), new Float32Array(6))).toThrow(/power-of-two/);
    expect(() => fft(new Float32Array(8), new Float32Array(4))).toThrow(/power-of-two/);
  });
});

describe("hann", () => {
  it("is periodic, starting at zero and peaking mid-window", () => {
    const window = hann(8);
    expect(window[0]).toBeCloseTo(0, 6);
    expect(window[4]).toBeCloseTo(1, 6);
    expect(window.at(-1)).toBeGreaterThan(0);
  });

  it("handles the degenerate lengths", () => {
    expect(hann(0)).toHaveLength(0);
    expect(Array.from(hann(1))).toEqual([1]);
  });
});

describe("coherentGain", () => {
  it("is half the length for a Hann window and the length for a rectangular one", () => {
    expect(coherentGain(hann(1024))).toBeCloseTo(512, 1);
    expect(coherentGain(new Float32Array(16).fill(1))).toBe(16);
    expect(coherentGain(new Float32Array(0))).toBe(0);
  });
});

describe("SpectrumAnalyzer", () => {
  it("reads a full-scale tone at a bin centre as 0 dBFS", () => {
    const analyzer = new SpectrumAnalyzer(256);
    const db = analyzer.powerDb(tone(256, 8), new Float32Array(256));
    expect(db[128 + 8]).toBeCloseTo(0, 1);
  });

  it("puts DC in the middle of the output", () => {
    const analyzer = new SpectrumAnalyzer(64);
    const db = analyzer.powerDb(tone(64, 0), new Float32Array(64));
    expect(db[32]).toBeCloseTo(0, 1);
    expect(db[0]).toBeLessThan(-40);
  });

  it("scales with amplitude the way dB says it should", () => {
    const analyzer = new SpectrumAnalyzer(256);
    const full = analyzer.powerDb(tone(256, 8), new Float32Array(256))[136] ?? 0;
    const half = analyzer.powerDb(tone(256, 8, 0.5), new Float32Array(256))[136] ?? 0;
    expect(full - half).toBeCloseTo(6.02, 1);
  });

  it("reads a negative offset below centre", () => {
    const analyzer = new SpectrumAnalyzer(128);
    const db = analyzer.powerDb(tone(128, -16), new Float32Array(128));
    expect(db[64 - 16]).toBeCloseTo(0, 1);
  });

  it("zero-pads a short burst rather than reading past it", () => {
    const analyzer = new SpectrumAnalyzer(128);
    const db = analyzer.powerDb(tone(32, 4), new Float32Array(128));
    expect(db.every((value) => Number.isFinite(value))).toBe(true);
    const peak = db.indexOf(Math.max(...db));
    expect(Math.abs(peak - (64 + 16))).toBeLessThanOrEqual(4);
  });

  it("reuses the caller's buffer when it fits", () => {
    const analyzer = new SpectrumAnalyzer(64);
    const out = new Float32Array(64);
    expect(analyzer.powerDb(tone(64, 1), out)).toBe(out);
    expect(analyzer.powerDb(tone(64, 1), new Float32Array(8))).toHaveLength(64);
  });

  it("floors an all-zero input instead of returning -Infinity", () => {
    const analyzer = new SpectrumAnalyzer(32);
    const db = analyzer.powerDb(new Float32Array(64), new Float32Array(32));
    expect(db.every((value) => Number.isFinite(value))).toBe(true);
    expect(db[16]).toBeLessThan(-200);
  });
});
