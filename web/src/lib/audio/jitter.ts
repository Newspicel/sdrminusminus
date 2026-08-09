// Jitter-buffer scheduling for audio playback (PLAN §9: 60–100 ms target). This class is
// injected into the AudioWorklet via `JitterBuffer.toString()` (see worklet.ts), so it must
// stay fully self-contained: no imports, no references to anything in module scope.
export class JitterBuffer {
  private readonly buf: Float32Array;
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

  constructor(targetSamples: number, maxSamples: number) {
    this.buf = new Float32Array(maxSamples);
    this.target = Math.min(targetSamples, maxSamples);
    this.trimAbove = Math.min(2 * this.target, maxSamples);
    this.trimHold = 10 * this.target;
  }

  get buffered(): number {
    return this.length;
  }

  /**
   * Append PCM. A push past capacity (tab sleep, network burst) sheds the oldest samples all
   * the way back to `target` — merely staying under the cap would park playback ~maxSamples
   * behind realtime for the rest of the stream. Sub-cap backlog that persists (repeated
   * bursts ratcheting the depth up) is shed the same way once it outlasts `trimHold`.
   */
  push(chunk: Float32Array): void {
    const cap = this.buf.length;
    const start = chunk.length > cap ? chunk.length - cap : 0;
    const n = chunk.length - start;
    if (this.length + n > cap) {
      this.dropOldest(this.length + n - this.target);
      this.trimStreak = 0;
    }
    for (let i = start; i < chunk.length; i++) {
      this.buf[this.writePos] = chunk[i] ?? 0;
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
   * Fill `out` with the next samples; silence while pre-buffering. An underrun (fewer buffered
   * samples than `out` needs) pads with silence and re-enters buffering until `target` is
   * reached again. Returns whether any real samples were written.
   */
  read(out: Float32Array): boolean {
    if (this.buffering) {
      out.fill(0);
      return false;
    }
    const cap = this.buf.length;
    const n = Math.min(out.length, this.length);
    for (let i = 0; i < n; i++) {
      out[i] = this.buf[this.readPos] ?? 0;
      this.readPos = (this.readPos + 1) % cap;
    }
    this.length -= n;
    if (n < out.length) {
      out.fill(0, n);
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
    this.readPos = (this.readPos + drop) % this.buf.length;
    this.length -= drop;
  }
}
