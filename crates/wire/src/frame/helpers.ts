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
