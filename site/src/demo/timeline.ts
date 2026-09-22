import type { RecordedMessage } from "../../../web/e2e/demoSession";

const HEADER_LEN = 16;

export interface Due<T> {
  item: T;
  lap: number;
}

export function due<T extends { at: number }>(
  items: readonly T[],
  loopMs: number,
  from: number,
  to: number,
): Due<T>[] {
  const out: Due<T>[] = [];
  if (loopMs <= 0 || to <= from) {
    return out;
  }
  for (let lap = Math.floor(from / loopMs); lap * loopMs < to; lap++) {
    for (const item of items) {
      const at = lap * loopMs + item.at;
      if (at >= from && at < to) {
        out.push({ item, lap });
      }
    }
  }
  return out;
}

export interface Frame {
  at: number;
  bytes: Uint8Array;
}

export interface FrameTrack {
  frames: Frame[];
  seqStep: number;
  timestampStep: bigint;
}

export function frameTrack(messages: readonly RecordedMessage[]): FrameTrack {
  const frames = messages
    .filter((message) => message.binary !== undefined)
    .map((message) => ({ at: message.at, bytes: decodeBase64(message.binary ?? "") }))
    .filter((frame) => frame.bytes.length >= HEADER_LEN);
  const first = frames[0];
  const last = frames.at(-1);
  const previous = frames.at(-2);
  if (first === undefined || last === undefined) {
    return { frames, seqStep: 0, timestampStep: 0n };
  }
  const seqStride = previous === undefined ? 1 : seqOf(last.bytes) - seqOf(previous.bytes);
  const timestampStride =
    previous === undefined ? 1n : timestampOf(last.bytes) - timestampOf(previous.bytes);
  const seqStep = seqOf(last.bytes) - seqOf(first.bytes) + seqStride;
  const timestampStep = timestampOf(last.bytes) - timestampOf(first.bytes) + timestampStride;
  return { frames, seqStep, timestampStep };
}

export function lapped(track: FrameTrack, frame: Frame, lap: number): ArrayBuffer {
  const out = frame.bytes.slice().buffer;
  const view = new DataView(out);
  view.setUint32(4, (view.getUint32(4, true) + lap * track.seqStep) >>> 0, true);
  view.setBigUint64(8, view.getBigUint64(8, true) + BigInt(lap) * track.timestampStep, true);
  return out;
}

function seqOf(bytes: Uint8Array): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(4, true);
}

function timestampOf(bytes: Uint8Array): bigint {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getBigUint64(8, true);
}

function decodeBase64(text: string): Uint8Array {
  const raw = atob(text);
  const bytes = new Uint8Array(raw.length);
  for (let index = 0; index < raw.length; index++) {
    bytes[index] = raw.charCodeAt(index);
  }
  return bytes;
}
