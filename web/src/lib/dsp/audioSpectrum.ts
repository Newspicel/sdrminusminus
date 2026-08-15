// The audio spectrogram: a voiceprint of what a channel is actually playing.
//
// A different question from the channel's passband. The passband shows what is on the air; this
// shows what came out of the demodulator — where CTCSS tones, RDS and stereo pilots, hum, clipping
// and the formants of a voice all live, none of which are visible before demodulation.
//
// Real input, so only the positive half of the transform is kept: a real signal's spectrum is
// symmetric, and drawing both halves would be drawing the same thing twice.

import { coherentGain, fft, hann } from "./fft";

/** Transform size. 1024 at 48 kHz is a ~47 Hz bin, which resolves a CTCSS tone from its
 * neighbours (they are spaced from about 3 Hz apart, but the tone's *presence* is the reading)
 * and keeps the row rate high enough that speech does not smear. */
export const AUDIO_FFT_SIZE = 1024;
/** Samples between rows. Half the window, the usual overlap for a spectrogram that has to look
 * continuous rather than strobed. */
export const AUDIO_HOP = 512;

/** The dB window rows are quantized over. Fixed, not adaptive: a voiceprint is read for its
 * shape over time, and a window that breathed with every syllable would destroy exactly that. */
export const AUDIO_DB_MIN = -90;
export const AUDIO_DB_MAX = 0;

/**
 * Turns a stream of decoded PCM into spectrogram rows.
 *
 * Feed it whatever blocks arrive; it emits a row every `hop` samples, so the row rate is a
 * property of the audio and not of how the decoder happened to chunk it.
 */
export class AudioSpectrogram {
  /** Bins in a row: the positive half of the transform, DC included. */
  readonly bins: number;
  private readonly size: number;
  private readonly hop: number;
  private readonly window: Float32Array;
  private readonly invGain: number;
  private readonly re: Float32Array;
  private readonly im: Float32Array;
  /** Ring of the most recent `size` mono samples, and where the next one goes. */
  private readonly history: Float32Array;
  private write = 0;
  /** Samples since the last row, against `hop`. */
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

  /**
   * Fold one decoded block in, emitting a row per completed hop.
   *
   * `pcm` is interleaved at `channels`; the channels are averaged. A stereo broadcast's two rails
   * carry the same programme, and a spectrogram of one of them would silently drop whatever the
   * other carried alone.
   *
   * The row handed to `emit` is reused between calls: a caller that keeps it must copy.
   */
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
    // Read oldest-first out of the ring, so the window sits over a contiguous span of audio.
    for (let i = 0; i < this.size; i++) {
      this.re[i] = (this.history[(this.write + i) % this.size] ?? 0) * (this.window[i] ?? 0);
      this.im[i] = 0;
    }
    fft(this.re, this.im);

    const span = AUDIO_DB_MAX - AUDIO_DB_MIN;
    for (let k = 0; k < this.bins; k++) {
      const rr = this.re[k] ?? 0;
      const ii = this.im[k] ?? 0;
      // Doubled except at DC: a real signal puts half of each tone's energy in the negative half
      // that is being discarded, and without this a full-scale tone would read 6 dB low.
      const fold = k === 0 ? 1 : 2;
      const mag = Math.sqrt(rr * rr + ii * ii) * this.invGain * fold;
      const db = 20 * Math.log10(mag + 1e-12);
      const t = (db - AUDIO_DB_MIN) / span;
      this.row[k] = Math.min(255, Math.max(0, Math.round(t * 255)));
    }
    return this.row;
  }
}

/** Frequency at the top of a row, which is what labels the axis. */
export function audioNyquistHz(sampleRate = 48_000): number {
  return sampleRate / 2;
}

/** Frequency of bin `k` of a row. */
export function audioBinHz(k: number, bins: number, sampleRate = 48_000): number {
  return bins <= 0 ? 0 : (k / bins) * audioNyquistHz(sampleRate);
}
