// Buffers are built by hand per the layout documented in `crates/wire/src/frame.rs`, so a
// drift between the Rust encoder and this decoder fails here, not in the browser.
import { describe, expect, it } from "vitest";
import {
  decodeAudio,
  decodeSpectrum,
  FRAME_KIND_AUDIO_OPUS,
  FRAME_KIND_SPECTRUM,
  frameKind,
  PROTOCOL_VERSION,
} from "./frame";

function header(buf: ArrayBuffer, kind: number, streamId: number, seq: number, timestamp: bigint) {
  const view = new DataView(buf);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, kind);
  view.setUint16(2, streamId, true);
  view.setUint32(4, seq, true);
  view.setBigUint64(8, timestamp, true);
  return view;
}

function spectrumBuffer(bins: Uint8Array): ArrayBuffer {
  const buf = new ArrayBuffer(16 + 22 + bins.length);
  const view = header(buf, FRAME_KIND_SPECTRUM, 7, 42, 1_000_000n);
  view.setFloat64(16, 100_300_000, true);
  view.setFloat32(24, 2_400_000, true);
  view.setFloat32(28, -120, true);
  view.setFloat32(32, -20, true);
  view.setUint16(36, bins.length, true);
  new Uint8Array(buf, 38).set(bins);
  return buf;
}

function audioBuffer(opus: Uint8Array): ArrayBuffer {
  const buf = new ArrayBuffer(16 + 1 + opus.length);
  const view = header(buf, FRAME_KIND_AUDIO_OPUS, 3, 512, 96_000n);
  view.setUint8(16, 1);
  new Uint8Array(buf, 17).set(opus);
  return buf;
}

describe("frameKind", () => {
  it("reads the kind byte", () => {
    expect(frameKind(spectrumBuffer(new Uint8Array(4)))).toBe(FRAME_KIND_SPECTRUM);
    expect(frameKind(audioBuffer(new Uint8Array(4)))).toBe(FRAME_KIND_AUDIO_OPUS);
  });

  it("rejects short buffers and unknown protocol versions", () => {
    expect(frameKind(new ArrayBuffer(15))).toBeNull();
    const buf = audioBuffer(new Uint8Array(4));
    new DataView(buf).setUint8(0, PROTOCOL_VERSION + 1);
    expect(frameKind(buf)).toBeNull();
  });
});

describe("decodeSpectrum", () => {
  it("decodes every field exactly", () => {
    const bins = Uint8Array.from({ length: 64 }, (_, i) => (i * 4) & 0xff);
    const frame = decodeSpectrum(spectrumBuffer(bins));
    expect(frame).not.toBeNull();
    expect(frame?.streamId).toBe(7);
    expect(frame?.seq).toBe(42);
    expect(frame?.timestamp).toBe(1_000_000n);
    expect(frame?.centerHz).toBe(100_300_000);
    expect(frame?.spanHz).toBe(2_400_000);
    expect(frame?.dbMin).toBe(-120);
    expect(frame?.dbMax).toBe(-20);
    expect(Array.from(frame?.bins ?? [])).toEqual(Array.from(bins));
  });

  it("rejects audio frames, wrong versions, and truncated bins", () => {
    expect(decodeSpectrum(audioBuffer(new Uint8Array(8)))).toBeNull();

    const wrongVersion = spectrumBuffer(new Uint8Array(4));
    new DataView(wrongVersion).setUint8(0, PROTOCOL_VERSION + 1);
    expect(decodeSpectrum(wrongVersion)).toBeNull();

    const truncated = spectrumBuffer(new Uint8Array(4)).slice(0, 39);
    expect(decodeSpectrum(truncated)).toBeNull();
  });
});

describe("decodeAudio", () => {
  it("decodes every field exactly", () => {
    const opus = Uint8Array.from({ length: 96 }, (_, i) => (i * 3) & 0xff);
    const frame = decodeAudio(audioBuffer(opus));
    expect(frame).not.toBeNull();
    expect(frame?.streamId).toBe(3);
    expect(frame?.seq).toBe(512);
    expect(frame?.timestamp).toBe(96_000n);
    expect(frame?.chLayout).toBe(1);
    expect(Array.from(frame?.opus ?? [])).toEqual(Array.from(opus));
  });

  it("accepts an empty opus payload but rejects a missing ch_layout byte", () => {
    const empty = decodeAudio(audioBuffer(new Uint8Array(0)));
    expect(empty?.opus.length).toBe(0);
    expect(decodeAudio(audioBuffer(new Uint8Array(0)).slice(0, 16))).toBeNull();
  });

  it("rejects spectrum frames and wrong versions", () => {
    expect(decodeAudio(spectrumBuffer(new Uint8Array(4)))).toBeNull();

    const wrongVersion = audioBuffer(new Uint8Array(8));
    new DataView(wrongVersion).setUint8(0, PROTOCOL_VERSION + 1);
    expect(decodeAudio(wrongVersion)).toBeNull();
  });
});
