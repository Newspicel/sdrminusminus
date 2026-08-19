export const PROTOCOL_VERSION = 1;
export const FRAME_KIND_SPECTRUM = 0;
export const FRAME_KIND_AUDIO_OPUS = 1;
export const FRAME_KIND_IQ_F32 = 2;
export const FRAME_KIND_VIDEO_GRAY = 3;
export const FRAME_KIND_VIDEO_RGB = 4;
export const FRAME_KIND_SYMBOLS = 5;
export const FRAME_KIND_RANGE_DOPPLER = 6;
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

export interface RangeDopplerFrame {
  streamId: number;
  seq: number;
  timestamp: bigint;
  ranges: number;
  dopplers: number;
  rangeStepUs: number;
  dopplerStepHz: number;
  dbMin: number;
  dbMax: number;
  cells: Uint8Array;
}

export function decodeRangeDoppler(buffer: ArrayBuffer): RangeDopplerFrame | null {
  if (buffer.byteLength < HEADER_LEN + 20) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION || view.getUint8(1) !== FRAME_KIND_RANGE_DOPPLER) {
    return null;
  }
  const streamId = view.getUint16(2, true);
  const seq = view.getUint32(4, true);
  const timestamp = view.getBigUint64(8, true);
  const ranges = view.getUint16(16, true);
  const dopplers = view.getUint16(18, true);
  const rangeStepUs = view.getFloat32(20, true);
  const dopplerStepHz = view.getFloat32(24, true);
  const dbMin = view.getFloat32(28, true);
  const dbMax = view.getFloat32(32, true);
  const cells = ranges * dopplers;
  if (cells === 0 || buffer.byteLength < 36 + cells) {
    return null;
  }
  return {
    streamId,
    seq,
    timestamp,
    ranges,
    dopplers,
    rangeStepUs,
    dopplerStepHz,
    dbMin,
    dbMax,
    cells: new Uint8Array(buffer, 36, cells),
  };
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

export type SymbolPlane = "complex" | "level";

export interface SymbolFrame {
  streamId: number;
  seq: number;
  timestamp: bigint;
  plane: SymbolPlane;
  symbolRate: number;
  evm: number;
  merDb: number;
  margin: number;
  freqErrorHz: number;
  reference: Float32Array;
  symbols: Float32Array;
}

const SYMBOL_BODY = 1 + 4 * 5 + 2;

export function decodeSymbols(buffer: ArrayBuffer): SymbolFrame | null {
  if (buffer.byteLength < HEADER_LEN + SYMBOL_BODY) {
    return null;
  }
  const view = new DataView(buffer);
  if (view.getUint8(0) !== PROTOCOL_VERSION || view.getUint8(1) !== FRAME_KIND_SYMBOLS) {
    return null;
  }
  const plane = view.getUint8(16);
  if (plane !== 0 && plane !== 1) {
    return null;
  }
  const referenceLen = view.getUint16(37, true);
  const at = HEADER_LEN + SYMBOL_BODY;
  const floats = Math.floor((buffer.byteLength - at) / 4);
  if (floats < referenceLen) {
    return null;
  }
  const read = (count: number, from: number): Float32Array => {
    const out = new Float32Array(count);
    for (let i = 0; i < count; i++) {
      out[i] = view.getFloat32(from + i * 4, true);
    }
    return out;
  };
  return {
    streamId: view.getUint16(2, true),
    seq: view.getUint32(4, true),
    timestamp: view.getBigUint64(8, true),
    plane: plane === 0 ? "complex" : "level",
    symbolRate: view.getFloat32(17, true),
    evm: view.getFloat32(21, true),
    merDb: view.getFloat32(25, true),
    margin: view.getFloat32(29, true),
    freqErrorHz: view.getFloat32(33, true),
    reference: read(referenceLen, at),
    symbols: read(floats - referenceLen, at + referenceLen * 4),
  };
}
