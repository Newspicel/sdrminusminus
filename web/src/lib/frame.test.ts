import { describe, expect, it } from "vitest";
import {
  decodeAudio,
  decodeRangeDoppler,
  decodeSpectrum,
  decodeSymbols,
  decodeVideo,
  FRAME_KIND_AUDIO_OPUS,
  FRAME_KIND_RANGE_DOPPLER,
  FRAME_KIND_SPECTRUM,
  FRAME_KIND_SYMBOLS,
  FRAME_KIND_VIDEO_GRAY,
  FRAME_KIND_VIDEO_RGB,
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

function audioBuffer(opus: Uint8Array, chLayout = 1): ArrayBuffer {
  const buf = new ArrayBuffer(16 + 1 + opus.length);
  const view = header(buf, FRAME_KIND_AUDIO_OPUS, 3, 512, 96_000n);
  view.setUint8(16, chLayout);
  new Uint8Array(buf, 17).set(opus);
  return buf;
}

function videoBuffer(
  width: number,
  height: number,
  pixels: Uint8Array,
  kind = FRAME_KIND_VIDEO_GRAY,
): ArrayBuffer {
  const buf = new ArrayBuffer(16 + 4 + pixels.length);
  const view = header(buf, kind, 0x8001, 9, 2_000_000n);
  view.setUint16(16, width, true);
  view.setUint16(18, height, true);
  new Uint8Array(buf, 20).set(pixels);
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

  it("reads the stereo layout without shifting the payload", () => {
    const opus = Uint8Array.from([9, 8, 7]);
    const frame = decodeAudio(audioBuffer(opus, 2));
    expect(frame?.chLayout).toBe(2);
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

describe("decodeVideo", () => {
  it("decodes every field exactly", () => {
    const luma = Uint8Array.from({ length: 8 * 4 }, (_, i) => (i * 7) & 0xff);
    const frame = decodeVideo(videoBuffer(8, 4, luma));
    expect(frame).not.toBeNull();
    expect(frame?.streamId).toBe(0x8001);
    expect(frame?.seq).toBe(9);
    expect(frame?.timestamp).toBe(2_000_000n);
    expect(frame?.width).toBe(8);
    expect(frame?.height).toBe(4);
    expect(frame?.format).toBe("gray");
    expect(Array.from(frame?.pixels ?? [])).toEqual(Array.from(luma));
  });

  it("decodes RGB pixels without changing their channel order", () => {
    const rgb = Uint8Array.from({ length: 2 * 2 * 3 }, (_, i) => i * 17);
    const frame = decodeVideo(videoBuffer(2, 2, rgb, FRAME_KIND_VIDEO_RGB));
    expect(frame?.format).toBe("rgb");
    expect(Array.from(frame?.pixels ?? [])).toEqual(Array.from(rgb));
  });

  it("rejects a payload that is shorter than its geometry, and an empty one", () => {
    const luma = new Uint8Array(8 * 4);
    expect(decodeVideo(videoBuffer(8, 4, luma).slice(0, 20 + 31))).toBeNull();
    expect(decodeVideo(videoBuffer(0, 0, new Uint8Array(0)))).toBeNull();
  });

  it("rejects other kinds and wrong versions", () => {
    expect(decodeVideo(audioBuffer(new Uint8Array(8)))).toBeNull();
    expect(decodeAudio(videoBuffer(2, 2, new Uint8Array(4)))).toBeNull();

    const wrongVersion = videoBuffer(2, 2, new Uint8Array(4));
    new DataView(wrongVersion).setUint8(0, PROTOCOL_VERSION + 1);
    expect(decodeVideo(wrongVersion)).toBeNull();
  });
});

function symbolBuffer(
  reference: readonly number[],
  symbols: readonly number[],
  plane = 0,
): ArrayBuffer {
  const buffer = new ArrayBuffer(39 + (reference.length + symbols.length) * 4);
  const view = new DataView(buffer);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, FRAME_KIND_SYMBOLS);
  view.setUint16(2, 300, true);
  view.setUint32(4, 9, true);
  view.setBigUint64(8, 4096n, true);
  view.setUint8(16, plane);
  view.setFloat32(17, 4800, true);
  view.setFloat32(21, 0.125, true);
  view.setFloat32(25, 18.06, true);
  view.setFloat32(29, 2.5, true);
  view.setFloat32(33, -12, true);
  view.setUint16(37, reference.length, true);
  [...reference, ...symbols].forEach((value, i) => {
    view.setFloat32(39 + i * 4, value, true);
  });
  return buffer;
}

describe("decodeSymbols", () => {
  it("splits the reference from the cloud that follows it", () => {
    const frame = decodeSymbols(symbolBuffer([1, 3, -1, -3], [1, -1, 3, 3, -3], 1));
    expect(frame?.streamId).toBe(300);
    expect(frame?.seq).toBe(9);
    expect(frame?.timestamp).toBe(4096n);
    expect(frame?.plane).toBe("level");
    expect(frame?.symbolRate).toBe(4800);
    expect(frame?.margin).toBe(2.5);
    expect(frame?.freqErrorHz).toBe(-12);
    expect(Array.from(frame?.reference ?? [])).toEqual([1, 3, -1, -3]);
    expect(Array.from(frame?.symbols ?? [])).toEqual([1, -1, 3, 3, -3]);
  });

  it("reads a complex plane as such", () => {
    const frame = decodeSymbols(symbolBuffer([1, 0, -1, 0], [0.9, 0.1], 0));
    expect(frame?.plane).toBe("complex");
  });

  it("refuses a plane it cannot draw", () => {
    expect(decodeSymbols(symbolBuffer([1], [1], 7))).toBeNull();
  });

  it("refuses a frame whose reference does not fit the payload", () => {
    const buffer = symbolBuffer([1, 3, -1, -3], [1, -1], 1);
    new DataView(buffer).setUint16(37, 400, true);
    expect(decodeSymbols(buffer)).toBeNull();
  });

  it("refuses other kinds, short buffers and wrong versions", () => {
    expect(decodeSymbols(audioBuffer(new Uint8Array(8)))).toBeNull();
    expect(decodeSymbols(new ArrayBuffer(20))).toBeNull();
    const wrongVersion = symbolBuffer([1], [1]);
    new DataView(wrongVersion).setUint8(0, PROTOCOL_VERSION + 1);
    expect(decodeSymbols(wrongVersion)).toBeNull();
  });
});

function encode(ranges: number, dopplers: number, cells: number[]): ArrayBuffer {
  const buffer = new ArrayBuffer(36 + cells.length);
  const view = new DataView(buffer);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, FRAME_KIND_RANGE_DOPPLER);
  view.setUint16(2, 9, true);
  view.setUint32(4, 3, true);
  view.setBigUint64(8, 4096n, true);
  view.setUint16(16, ranges, true);
  view.setUint16(18, dopplers, true);
  view.setFloat32(20, 0.5, true);
  view.setFloat32(24, 4.25, true);
  view.setFloat32(28, -60, true);
  view.setFloat32(32, 0, true);
  new Uint8Array(buffer, 36).set(cells);
  return buffer;
}

describe("decodeRangeDoppler", () => {
  it("reads the shape back out of a surface frame", () => {
    const frame = decodeRangeDoppler(encode(4, 3, [...Array(12).keys()]));
    expect(frame).not.toBeNull();
    expect(frame?.streamId).toBe(9);
    expect(frame?.ranges).toBe(4);
    expect(frame?.dopplers).toBe(3);
    expect(frame?.rangeStepUs).toBe(0.5);
    expect(frame?.dopplerStepHz).toBe(4.25);
    expect(frame?.dbMin).toBe(-60);
    expect([...(frame?.cells ?? [])]).toEqual([...Array(12).keys()]);
  });

  it("refuses a frame that is shorter than the shape it declares", () => {
    const buffer = encode(4, 3, [1, 2, 3]);
    expect(decodeRangeDoppler(buffer)).toBeNull();
  });

  it("refuses a frame of another kind", () => {
    const buffer = encode(2, 1, [1, 2]);
    new DataView(buffer).setUint8(1, FRAME_KIND_SPECTRUM);
    expect(decodeRangeDoppler(buffer)).toBeNull();
  });
});
