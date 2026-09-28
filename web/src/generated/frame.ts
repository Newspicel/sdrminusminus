export const PROTOCOL_VERSION = 1;
const HEADER_LEN = 16;
export const FUSION_FRAME_CELLS = 128;
export const WS_SUBPROTOCOL = "sdrmm";
export const WS_BEARER_PROTOCOL_PREFIX = "sdrmm.bearer.";
export const FRAME_KIND_SPECTRUM = 0;
export const FRAME_KIND_AUDIO_OPUS = 1;
export const FRAME_KIND_IQ_F32 = 2;
export const FRAME_KIND_VIDEO_GRAY = 3;
export const FRAME_KIND_VIDEO_RGB = 4;
export const FRAME_KIND_SYMBOLS = 5;
export const FRAME_KIND_RANGE_DOPPLER = 6;
export const FRAME_KIND_SPATIAL_SPECTRUM = 7;
export const FRAME_KIND_VISIBILITY = 8;
export const FRAME_KIND_FUSION_GRID = 9;
export type SymbolPlane = "complex" | "level";
export function frameKind(buffer: ArrayBuffer): number | null {
  if (buffer.byteLength < HEADER_LEN) return null;
  const view = new DataView(buffer);
  return view.getUint8(0) === PROTOCOL_VERSION ? view.getUint8(1) : null;
}
class FrameReader {
  private view: DataView;
  private at = 2;
  valid: boolean;
  constructor(private buffer: ArrayBuffer, kinds: number[]) {
    this.view = new DataView(buffer);
    const kind = frameKind(buffer);
    this.valid = kind !== null && kinds.includes(kind);
  }
  get complete(): boolean { return this.valid && this.at === this.buffer.byteLength; }
  private take(size: number): number {
    const at = this.at;
    if (!Number.isSafeInteger(size) || size < 0 || at + size > this.buffer.byteLength) {
      this.valid = false;
      return -1;
    }
    this.at += size;
    return at;
  }
  u8(): number { const at = this.take(1); return at < 0 ? 0 : this.view.getUint8(at); }
  u16(): number { const at = this.take(2); return at < 0 ? 0 : this.view.getUint16(at, true); }
  u32(): number { const at = this.take(4); return at < 0 ? 0 : this.view.getUint32(at, true); }
  u64(): bigint { const at = this.take(8); return at < 0 ? 0n : this.view.getBigUint64(at, true); }
  f32(): number { const at = this.take(4); return at < 0 ? 0 : this.view.getFloat32(at, true); }
  f64(): number { const at = this.take(8); return at < 0 ? 0 : this.view.getFloat64(at, true); }
  plane(): SymbolPlane {
    const plane = this.u8();
    if (plane !== 0 && plane !== 1) this.valid = false;
    return plane === 0 ? "complex" : "level";
  }
  bytes(count = this.buffer.byteLength - this.at): Uint8Array {
    const at = this.take(count);
    return at < 0 ? new Uint8Array(0) : new Uint8Array(this.buffer, at, count);
  }
  floats(count = (this.buffer.byteLength - this.at) / 4): Float32Array {
    const at = this.take(count * 4);
    if (at < 0 || !Number.isSafeInteger(count)) { this.valid = false; return new Float32Array(0); }
    const out = new Float32Array(count);
    for (let index = 0; index < count; index++) out[index] = this.view.getFloat32(at + index * 4, true);
    return out;
  }
}
export interface SpectrumFrame {
 streamId: number; seq: number; timestamp: bigint;
centerHz: number;
spanHz: number;
dbMin: number;
dbMax: number;
bins: Uint8Array;
}
export function decodeSpectrum(buffer: ArrayBuffer): SpectrumFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_SPECTRUM]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const centerHz = reader.f64();
const spanHz = reader.f32();
const dbMin = reader.f32();
const dbMax = reader.f32();
const bins = reader.bytes(reader.u16());
if (!reader.complete) return null;
return { streamId, seq, timestamp, centerHz, spanHz, dbMin, dbMax, bins };
}
export interface AudioFrame {
 streamId: number; seq: number; timestamp: bigint;
chLayout: number;
opus: Uint8Array;
}
export function decodeAudio(buffer: ArrayBuffer): AudioFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_AUDIO_OPUS]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const chLayout = reader.u8();
const opus = reader.bytes();
if (!reader.complete) return null;
return { streamId, seq, timestamp, chLayout, opus };
}
export interface IqFrame {
 streamId: number; seq: number; timestamp: bigint;
centerHz: number;
sampleRate: number;
samples: Float32Array;
}
export function decodeIq(buffer: ArrayBuffer): IqFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_IQ_F32]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const centerHz = reader.f64();
const sampleRate = reader.f32();
const samples = reader.floats();
if (samples.length === 0 || samples.length % 2 !== 0) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, centerHz, sampleRate, samples };
}
export interface SymbolFrame {
 streamId: number; seq: number; timestamp: bigint;
plane: SymbolPlane;
symbolRate: number;
evm: number;
merDb: number;
margin: number;
freqErrorHz: number;
reference: Float32Array;
symbols: Float32Array;
}
export function decodeSymbols(buffer: ArrayBuffer): SymbolFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_SYMBOLS]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const plane = reader.plane();
const symbolRate = reader.f32();
const evm = reader.f32();
const merDb = reader.f32();
const margin = reader.f32();
const freqErrorHz = reader.f32();
const reference = reader.floats(reader.u16());
const symbols = reader.floats();
if (!reader.complete) return null;
return { streamId, seq, timestamp, plane, symbolRate, evm, merDb, margin, freqErrorHz, reference, symbols };
}
export interface RangeDopplerFrame {
 streamId: number; seq: number; timestamp: bigint;
ranges: number;
dopplers: number;
rangeFirstM: number;
rangeStepM: number;
dopplerFirstHz: number;
dopplerStepHz: number;
carrierHz: number;
dbMin: number;
dbMax: number;
cells: Uint8Array;
}
export function decodeRangeDoppler(buffer: ArrayBuffer): RangeDopplerFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_RANGE_DOPPLER]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const ranges = reader.u16();
const dopplers = reader.u16();
const rangeFirstM = reader.f32();
const rangeStepM = reader.f32();
const dopplerFirstHz = reader.f32();
const dopplerStepHz = reader.f32();
const carrierHz = reader.f64();
const dbMin = reader.f32();
const dbMax = reader.f32();
const cells = reader.bytes(ranges * dopplers);
if (cells.length === 0 || cells.length !== ranges * dopplers) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, ranges, dopplers, rangeFirstM, rangeStepM, dopplerFirstHz, dopplerStepHz, carrierHz, dbMin, dbMax, cells };
}
export interface VideoFrame {
 streamId: number; seq: number; timestamp: bigint;
width: number;
height: number;
format: "gray" | "rgb"; pixels: Uint8Array;
}
export function decodeVideo(buffer: ArrayBuffer): VideoFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_VIDEO_GRAY, FRAME_KIND_VIDEO_RGB]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const width = reader.u16();
const height = reader.u16();
const format = frameKind(buffer) === FRAME_KIND_VIDEO_RGB ? "rgb" : "gray";
const pixels = reader.bytes(width * height * (format === "rgb" ? 3 : 1));
if (pixels.length === 0) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, width, height, format, pixels };
}
export interface SpatialSpectrumFrame {
 streamId: number; seq: number; timestamp: bigint;
centerHz: number;
spanHz: number;
bearings: number;
bins: number;
dbMin: number;
dbMax: number;
cells: Uint8Array;
}
export function decodeSpatialSpectrum(buffer: ArrayBuffer): SpatialSpectrumFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_SPATIAL_SPECTRUM]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const centerHz = reader.f64();
const spanHz = reader.f32();
const bearings = reader.u16();
const bins = reader.u16();
const dbMin = reader.f32();
const dbMax = reader.f32();
const cells = reader.bytes(bearings * bins);
if (cells.length === 0 || cells.length !== bearings * bins) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, centerHz, spanHz, bearings, bins, dbMin, dbMax, cells };
}
export interface VisibilityFrame {
 streamId: number; seq: number; timestamp: bigint;
centerHz: number;
spanHz: number;
baselines: number;
bins: number;
dbMin: number;
dbMax: number;
amplitude: Uint8Array;
phase: Uint8Array;
}
export function decodeVisibility(buffer: ArrayBuffer): VisibilityFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_VISIBILITY]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const centerHz = reader.f64();
const spanHz = reader.f32();
const baselines = reader.u16();
const bins = reader.u16();
const dbMin = reader.f32();
const dbMax = reader.f32();
const amplitude = reader.bytes(reader.u16());
if (amplitude.length === 0 || amplitude.length !== baselines * bins) return null;
const phase = reader.bytes(baselines * bins);
if (phase.length === 0 || phase.length !== baselines * bins) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, centerHz, spanHz, baselines, bins, dbMin, dbMax, amplitude, phase };
}
export interface FusionGridFrame {
 streamId: number; seq: number; timestamp: bigint;
south: number;
west: number;
north: number;
east: number;
cols: number;
rows: number;
cells: Uint8Array;
}
export function decodeFusionGrid(buffer: ArrayBuffer): FusionGridFrame | null {
const reader = new FrameReader(buffer, [FRAME_KIND_FUSION_GRID]);
if (!reader.valid) return null;
const streamId = reader.u16();
const seq = reader.u32();
const timestamp = reader.u64();
const south = reader.f64();
const west = reader.f64();
const north = reader.f64();
const east = reader.f64();
const cols = reader.u16();
const rows = reader.u16();
const cells = reader.bytes(cols * rows);
if (cells.length === 0 || cells.length !== cols * rows) return null;
if (cols > FUSION_FRAME_CELLS || rows > FUSION_FRAME_CELLS) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, south, west, north, east, cols, rows, cells };
}
