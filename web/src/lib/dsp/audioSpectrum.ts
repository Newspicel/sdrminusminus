import { coherentGain, fft, hann } from "./fft";

export const AUDIO_FFT_SIZE = 1024;
export const AUDIO_HOP = 512;

export const AUDIO_DB_MIN = -90;
export const AUDIO_DB_MAX = 0;

export class AudioSpectrogram {
  readonly bins: number;
  private readonly size: number;
  private readonly hop: number;
  private readonly window: Float32Array;
  private readonly invGain: number;
  private readonly re: Float32Array;
  private readonly im: Float32Array;
  private readonly history: Float32Array;
  private write = 0;
  private since = 0;
  private readonly row: Uint8Array;

  constructor(size = AUDIO_FFT_SIZE, hop = AUDIO_HOP) {
    this.size = size;
    this.hop = Math.max(1, hop);
    this.bins = size / 2;
    this.window = hann(size);
    this.invGain = 1 / Math.max(coherentGain(this.window), Number.MIN_VALUE);
    this.re = new Float32Array(size);
    this.im = new Float32Array(size);
    this.history = new Float32Array(size);
    this.row = new Uint8Array(this.bins);
  }

  push(pcm: Float32Array, channels: number, emit: (row: Uint8Array) => void): void {
    const lanes = Math.max(1, Math.floor(channels));
    const frames = Math.floor(pcm.length / lanes);
    for (let frame = 0; frame < frames; frame++) {
      let sum = 0;
      for (let lane = 0; lane < lanes; lane++) {
        sum += pcm[frame * lanes + lane] ?? 0;
      }
      this.history[this.write] = sum / lanes;
      this.write = (this.write + 1) % this.size;
      this.since += 1;
      if (this.since >= this.hop) {
        this.since = 0;
        emit(this.transform());
      }
    }
  }

  private transform(): Uint8Array {
    for (let i = 0; i < this.size; i++) {
      this.re[i] = (this.history[(this.write + i) % this.size] ?? 0) * (this.window[i] ?? 0);
      this.im[i] = 0;
    }
    fft(this.re, this.im);

    const span = AUDIO_DB_MAX - AUDIO_DB_MIN;
    for (let k = 0; k < this.bins; k++) {
      const rr = this.re[k] ?? 0;
      const ii = this.im[k] ?? 0;
      const fold = k === 0 ? 1 : 2;
      const mag = Math.sqrt(rr * rr + ii * ii) * this.invGain * fold;
      const db = 20 * Math.log10(mag + 1e-12);
      const t = (db - AUDIO_DB_MIN) / span;
      this.row[k] = Math.min(255, Math.max(0, Math.round(t * 255)));
    }
    return this.row;
  }
}

export function audioNyquistHz(sampleRate = 48_000): number {
  return sampleRate / 2;
}

export function audioBinHz(k: number, bins: number, sampleRate = 48_000): number {
  return bins <= 0 ? 0 : (k / bins) * audioNyquistHz(sampleRate);
}
