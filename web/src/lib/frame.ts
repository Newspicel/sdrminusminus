export const PROTOCOL_VERSION = 1;
export const FRAME_KIND_SPECTRUM = 0;
export const FRAME_KIND_AUDIO_OPUS = 1;
export const FRAME_KIND_IQ_F32 = 2;
export const FRAME_KIND_VIDEO_GRAY = 3;
const HEADER_LEN = 16;

/** The `kind` header byte, or null if the buffer can't be a frame we understand. */
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

/** Decode a SPECTRUM frame, or return null if the buffer is not one we understand. */
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
  /** Layout of this packet: 1 = mono, 2 = stereo. A channel may switch between them. */
  chLayout: number;
  /** One Opus packet: byte `HEADER_LEN + 1` to the end of the WS frame. */
  opus: Uint8Array;
}

/** Decode an AUDIO_OPUS frame, or return null if the buffer is not one we understand. */
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
  /** 8-bit luma, row-major from the top line, exactly `width · height` bytes. */
  luma: Uint8Array;
}

/** Decode a VIDEO_GRAY frame, or return null if the buffer is not one we understand. */
export function decodeVideo(buffer: ArrayBuffer): VideoFrame | null {
  if (buffer.byteLength < HEADER_LEN + 4) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION || view.getUint8(1) !== FRAME_KIND_VIDEO_GRAY) {
    return null;
  }
  const streamId = view.getUint16(2, true);
  const seq = view.getUint32(4, true);
  const timestamp = view.getBigUint64(8, true);
  const width = view.getUint16(16, true);
  const height = view.getUint16(18, true);
  // Geometry and payload must agree: a canvas sized from the header and filled from a short
  // payload would draw the previous picture's tail as this one's bottom rows.
  const pixels = width * height;
  if (pixels === 0 || buffer.byteLength < HEADER_LEN + 4 + pixels) {
    return null;
  }
  const luma = new Uint8Array(buffer, HEADER_LEN + 4, pixels);
  return { streamId, seq, timestamp, width, height, luma };
}

export interface IqFrame {
  streamId: number;
  seq: number;
  /** Channel-rate sample count at the first sample of this burst. */
  timestamp: bigint;
  /** The channel's own rate — the bandwidth this baseband spans. */
  sampleRate: number;
  /** Absolute frequency the baseband is centred on. */
  centerHz: number;
  /** Interleaved I, Q. Always an even length: `samples.length / 2` complex samples. */
  samples: Float32Array;
}

/** Decode an IQ_F32 frame, or return null if the buffer is not one we understand. */
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
  // An odd component count would leave a reader one component short of a complex sample; the
  // truncation is what keeps every consumer able to step the array in pairs without checking.
  const components = Math.floor((buffer.byteLength - (HEADER_LEN + 12)) / 4) & ~1;
  if (components === 0) {
    return null;
  }
  // Copied rather than viewed: the socket's buffer is reused, and a burst outlives the message
  // it arrived in — a display holds the last one until the next replaces it.
  const samples = new Float32Array(components);
  for (let i = 0; i < components; i++) {
    samples[i] = view.getFloat32(HEADER_LEN + 12 + i * 4, true);
  }
  return { streamId, seq, timestamp, sampleRate, centerHz, samples };
}
