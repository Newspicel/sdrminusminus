// Binary WS frame decoder — the deliberate hand-synced counterpart to `crates/wire/src/frame.rs`
// (PLAN §4). Keep the two in lockstep; all fields little-endian.

export const PROTOCOL_VERSION = 1;
export const FRAME_KIND_SPECTRUM = 0;
export const FRAME_KIND_AUDIO_OPUS = 1;
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
  /** 48 kHz-domain sample-frame count since the channel's audio started (PLAN §5). */
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
  /** Channel-rate sample count when the picture completed (PLAN §5). */
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
