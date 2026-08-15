export const PROTOCOL_VERSION = 1;
export const FRAME_KIND_SPECTRUM = 0;
export const FRAME_KIND_AUDIO_OPUS = 1;
export const FRAME_KIND_IQ_F32 = 2;
export const FRAME_KIND_VIDEO_GRAY = 3;
export const FRAME_KIND_VIDEO_RGB = 4;
const HEADER_LEN = 16;

export function frameKind(buffer: ArrayBuffer): number | null {
  if (buffer.byteLength < HEADER_LEN) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION) {
    return null;
  }
  return view.getUint8(1);
}

export interface SpectrumFrame {
  streamId: number;
  seq: number;
  timestamp: bigint;
  centerHz: number;
  spanHz: number;
  dbMin: number;
  dbMax: number;
  bins: Uint8Array;
}

export function decodeSpectrum(buffer: ArrayBuffer): SpectrumFrame | null {
  if (buffer.byteLength < HEADER_LEN + 22) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION || view.getUint8(1) !== FRAME_KIND_SPECTRUM) {
    return null;
  }
  const streamId = view.getUint16(2, true);
  const seq = view.getUint32(4, true);
  const timestamp = view.getBigUint64(8, true);
  const centerHz = view.getFloat64(16, true);
  const spanHz = view.getFloat32(24, true);
  const dbMin = view.getFloat32(28, true);
  const dbMax = view.getFloat32(32, true);
  const n = view.getUint16(36, true);
  if (buffer.byteLength < 38 + n) {
    return null;
  }
  const bins = new Uint8Array(buffer, 38, n);
  return { streamId, seq, timestamp, centerHz, spanHz, dbMin, dbMax, bins };
}

export interface AudioFrame {
  streamId: number;
  seq: number;
  timestamp: bigint;
  chLayout: number;
  opus: Uint8Array;
}

export function decodeAudio(buffer: ArrayBuffer): AudioFrame | null {
  if (buffer.byteLength < HEADER_LEN + 1) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION || view.getUint8(1) !== FRAME_KIND_AUDIO_OPUS) {
    return null;
  }
  const streamId = view.getUint16(2, true);
  const seq = view.getUint32(4, true);
  const timestamp = view.getBigUint64(8, true);
  const chLayout = view.getUint8(16);
  const opus = new Uint8Array(buffer, HEADER_LEN + 1);
  return { streamId, seq, timestamp, chLayout, opus };
}

export interface VideoFrame {
  streamId: number;
  seq: number;
  timestamp: bigint;
  width: number;
  height: number;
  format: "gray" | "rgb";
  pixels: Uint8Array;
}

export function decodeVideo(buffer: ArrayBuffer): VideoFrame | null {
  if (buffer.byteLength < HEADER_LEN + 4) {
    return null;
  }
  const view = new DataView(buffer);
  const kind = view.getUint8(1);
  if (
    view.getUint8(0) !== PROTOCOL_VERSION ||
    (kind !== FRAME_KIND_VIDEO_GRAY && kind !== FRAME_KIND_VIDEO_RGB)
  ) {
    return null;
  }
  const streamId = view.getUint16(2, true);
  const seq = view.getUint32(4, true);
  const timestamp = view.getBigUint64(8, true);
  const width = view.getUint16(16, true);
  const height = view.getUint16(18, true);
  const format = kind === FRAME_KIND_VIDEO_RGB ? "rgb" : "gray";
  const bytes = width * height * (format === "rgb" ? 3 : 1);
  if (bytes === 0 || buffer.byteLength < HEADER_LEN + 4 + bytes) {
    return null;
  }
  const pixels = new Uint8Array(buffer, HEADER_LEN + 4, bytes);
  return { streamId, seq, timestamp, width, height, format, pixels };
}

export interface IqFrame {
  streamId: number;
  seq: number;
  timestamp: bigint;
  sampleRate: number;
  centerHz: number;
  samples: Float32Array;
}

export function decodeIq(buffer: ArrayBuffer): IqFrame | null {
  if (buffer.byteLength < HEADER_LEN + 12) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION || view.getUint8(1) !== FRAME_KIND_IQ_F32) {
    return null;
  }
  const streamId = view.getUint16(2, true);
  const seq = view.getUint32(4, true);
  const timestamp = view.getBigUint64(8, true);
  const centerHz = view.getFloat64(16, true);
  const sampleRate = view.getFloat32(24, true);
  const components = Math.floor((buffer.byteLength - (HEADER_LEN + 12)) / 4) & ~1;
  if (components === 0) {
    return null;
  }
  const samples = new Float32Array(components);
  for (let i = 0; i < components; i++) {
    samples[i] = view.getFloat32(HEADER_LEN + 12 + i * 4, true);
  }
  return { streamId, seq, timestamp, sampleRate, centerHz, samples };
}
