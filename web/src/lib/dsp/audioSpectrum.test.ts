import { describe, expect, it } from "vitest";
import {
  AUDIO_DB_MAX,
  AUDIO_DB_MIN,
  AudioSpectrogram,
  audioBinHz,
  audioNyquistHz,
} from "./audioSpectrum";

const RATE = 48_000;

/** Interleaved PCM of a real sine at `hz`, `frames` long. */
function tone(hz: number, frames: number, channels = 1, amp = 1): Float32Array {
  const pcm = new Float32Array(frames * channels);
  for (let frame = 0; frame < frames; frame++) {
    const value = Math.sin((2 * Math.PI * hz * frame) / RATE) * amp;
    for (let lane = 0; lane < channels; lane++) {
      pcm[frame * channels + lane] = value;
    }
  }
  return pcm;
}

/** The last row a run of `pcm` produced. */
function lastRow(spectrogram: AudioSpectrogram, pcm: Float32Array, channels = 1): Uint8Array {
  let row: Uint8Array | null = null;
  spectrogram.push(pcm, channels, (emitted) => {
    row = Uint8Array.from(emitted);
  });
  if (row === null) {
    throw new Error("no row was emitted");
  }
  return row;
}

/** Read a row byte back to dB. */
function db(byte: number): number {
  return AUDIO_DB_MIN + (byte / 255) * (AUDIO_DB_MAX - AUDIO_DB_MIN);
}

describe("AudioSpectrogram", () => {
  it("emits one row per hop, whatever size the blocks arrive in", () => {
    const spectrogram = new AudioSpectrogram(256, 128);
    let rows = 0;
    // 1024 frames delivered as eight blocks of 128 is eight hops.
    for (let i = 0; i < 8; i++) {
      spectrogram.push(tone(1000, 128), 1, () => {
        rows += 1;
      });
    }
    expect(rows).toBe(8);
  });

  it("puts a tone in the bin its frequency belongs to", () => {
    const spectrogram = new AudioSpectrogram(1024, 512);
    // 3 kHz of 24 kHz across 512 bins is bin 64.
    const row = lastRow(spectrogram, tone(3000, 4096));
    const peak = row.indexOf(Math.max(...row));
    expect(Math.abs(peak - 64)).toBeLessThanOrEqual(1);
  });

  it("reads a full-scale tone at the top of its dB window", () => {
    const spectrogram = new AudioSpectrogram(1024, 512);
    const row = lastRow(spectrogram, tone(3000, 4096));
    expect(db(Math.max(...row))).toBeGreaterThan(-3);
  });

  it("scales with amplitude the way dB says it should", () => {
    const full = db(Math.max(...lastRow(new AudioSpectrogram(1024, 512), tone(3000, 4096))));
    const half = db(
      Math.max(...lastRow(new AudioSpectrogram(1024, 512), tone(3000, 4096, 1, 0.5))),
    );
    expect(full - half).toBeCloseTo(6.02, 0);
  });

  it("averages the channels of a stereo block", () => {
    const mono = lastRow(new AudioSpectrogram(1024, 512), tone(3000, 4096, 1));
    const stereo = lastRow(new AudioSpectrogram(1024, 512), tone(3000, 4096, 2), 2);
    expect(Math.max(...stereo)).toBeCloseTo(Math.max(...mono), -1);
  });

  it("floors silence rather than reading noise into it", () => {
    const spectrogram = new AudioSpectrogram(256, 128);
    const row = lastRow(spectrogram, new Float32Array(1024));
    expect(Math.max(...row)).toBe(0);
  });

  it("keeps only the positive half of the transform", () => {
    const spectrogram = new AudioSpectrogram(1024, 512);
    expect(spectrogram.bins).toBe(512);
    expect(lastRow(spectrogram, tone(3000, 4096))).toHaveLength(512);
  });

  it("spans a hop even when the audio arrives one sample at a time", () => {
    const spectrogram = new AudioSpectrogram(256, 128);
    const pcm = tone(3000, 256);
    let rows = 0;
    for (const sample of pcm) {
      spectrogram.push(Float32Array.of(sample), 1, () => {
        rows += 1;
      });
    }
    expect(rows).toBe(2);
  });

  it("treats a nonsense channel count as mono rather than dividing by it", () => {
    const spectrogram = new AudioSpectrogram(256, 128);
    expect(() => lastRow(spectrogram, tone(3000, 1024), 0)).not.toThrow();
  });
});

describe("axis helpers", () => {
  it("labels the top of a row at Nyquist", () => {
    expect(audioNyquistHz(RATE)).toBe(24_000);
    expect(audioBinHz(512, 512, RATE)).toBe(24_000);
  });

  it("places a bin at its share of the band", () => {
    expect(audioBinHz(0, 512, RATE)).toBe(0);
    expect(audioBinHz(64, 512, RATE)).toBe(3000);
  });

  it("has nothing to say about an empty row", () => {
    expect(audioBinHz(0, 0, RATE)).toBe(0);
  });
});
