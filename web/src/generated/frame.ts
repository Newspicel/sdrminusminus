export const PROTOCOL_VERSION = 1;
const HEADER_LEN = 16;
export const FRAME_KIND_SPECTRUM = 0;
export const FRAME_KIND_AUDIO_OPUS = 1;
export const FRAME_KIND_IQ_F32 = 2;
export const FRAME_KIND_VIDEO_GRAY = 3;
export const FRAME_KIND_VIDEO_RGB = 4;
export const FRAME_KIND_SYMBOLS = 5;
export const FRAME_KIND_RANGE_DOPPLER = 6;
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
rangeStepUs: number;
dopplerStepHz: number;
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
const rangeStepUs = reader.f32();
const dopplerStepHz = reader.f32();
const dbMin = reader.f32();
const dbMax = reader.f32();
const cells = reader.bytes(ranges * dopplers);
if (cells.length === 0) return null;
if (!reader.complete) return null;
return { streamId, seq, timestamp, ranges, dopplers, rangeStepUs, dopplerStepHz, dbMin, dbMax, cells };
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
