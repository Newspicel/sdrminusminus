// Jitter-buffer scheduling for audio playback (PLAN §9: 60–100 ms target). This class is
// injected into the AudioWorklet via `JitterBuffer.toString()` (see worklet.ts), so it must
// stay fully self-contained: no imports, no references to anything in module scope.
//
// Everything is counted in sample frames, never in samples: the ring holds interleaved audio
// and `read` deinterleaves it into one output per channel, so depth and timing mean the same
// thing whatever layout the stream is in.
export class JitterBuffer {
  private readonly buf: Float32Array;
  private readonly channels: number;
  /** Ring size in frames. */
  private readonly capacity: number;
  private readonly target: number;
  /** Depth above which a sustained backlog counts as latency to shed, not jitter headroom. */
  private readonly trimAbove: number;
  /** Audio that must arrive while above `trimAbove` before shedding (~1 s at 100 ms target). */
  private readonly trimHold: number;
  private readPos = 0;
  private writePos = 0;
  private length = 0;
  private buffering = true;
  private trimStreak = 0;

  constructor(targetFrames: number, maxFrames: number, channels: number) {
    this.channels = channels;
    this.capacity = maxFrames;
    this.buf = new Float32Array(maxFrames * channels);
    this.target = Math.min(targetFrames, maxFrames);
    this.trimAbove = Math.min(2 * this.target, maxFrames);
    this.trimHold = 10 * this.target;
  }

  /** Buffered depth, in sample frames. */
  get buffered(): number {
    return this.length;
  }

  /**
   * Append interleaved PCM. A push past capacity (tab sleep, network burst) sheds the oldest
   * frames all the way back to `target` — merely staying under the cap would park playback
   * ~maxFrames behind realtime for the rest of the stream. Sub-cap backlog that persists
   * (repeated bursts ratcheting the depth up) is shed the same way once it outlasts `trimHold`.
   */
  push(chunk: Float32Array): void {
    const cap = this.capacity;
    const ch = this.channels;
    const frames = Math.floor(chunk.length / ch);
    const start = frames > cap ? frames - cap : 0;
    const n = frames - start;
    if (this.length + n > cap) {
      this.dropOldest(this.length + n - this.target);
      this.trimStreak = 0;
    }
    for (let f = start; f < frames; f++) {
      const src = f * ch;
      const dst = this.writePos * ch;
      for (let c = 0; c < ch; c++) {
        this.buf[dst + c] = chunk[src + c] ?? 0;
      }
      this.writePos = (this.writePos + 1) % cap;
    }
    this.length += n;
    if (this.length >= this.target) {
      this.buffering = false;
    }
    if (this.length > this.trimAbove) {
      this.trimStreak += n;
      if (this.trimStreak >= this.trimHold) {
        this.dropOldest(this.length - this.target);
        this.trimStreak = 0;
      }
    } else {
      this.trimStreak = 0;
    }
  }

  /**
   * Fill one output buffer per channel with the next frames; silence while pre-buffering. An
   * underrun (fewer buffered frames than the outputs need) pads with silence and re-enters
   * buffering until `target` is reached again. Returns whether any real frames were written.
   */
  read(outputs: Float32Array[]): boolean {
    const wanted = outputs[0]?.length ?? 0;
    if (this.buffering) {
      for (const out of outputs) {
        out.fill(0);
      }
      return false;
    }
    const cap = this.capacity;
    const ch = this.channels;
    const n = Math.min(wanted, this.length);
    for (let c = 0; c < outputs.length; c++) {
      const out = outputs[c];
      if (!out) {
        continue;
      }
      // A stream with fewer channels than the output has feeds its last channel to the rest.
      const lane = Math.min(c, ch - 1);
      let pos = this.readPos;
      for (let f = 0; f < n; f++) {
        out[f] = this.buf[pos * ch + lane] ?? 0;
        pos = (pos + 1) % cap;
      }
      if (n < wanted) {
        out.fill(0, n);
      }
    }
    this.readPos = (this.readPos + n) % cap;
    this.length -= n;
    if (n < wanted) {
      this.buffering = true;
    }
    return n > 0;
  }

  clear(): void {
    this.readPos = 0;
    this.writePos = 0;
    this.length = 0;
    this.buffering = true;
    this.trimStreak = 0;
  }

  private dropOldest(count: number): void {
    const drop = Math.min(count, this.length);
    if (drop <= 0) {
      return;
    }
    this.readPos = (this.readPos + drop) % this.capacity;
    this.length -= drop;
  }
}
