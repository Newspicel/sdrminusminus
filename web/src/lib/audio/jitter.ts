export class JitterBuffer {
  private readonly buf: Float32Array;
  private readonly channels: number;
  /** Ring size in frames. */
  private readonly capacity: number;
  /** The configured pre-buffer: the smallest target adaptation may fall back to. */
  private readonly floor: number;
  /** The largest target underruns may grow it to. */
  private readonly ceiling: number;
  /** One step of target growth or relaxation. */
  private readonly step: number;
  /** Smoothed depth this far above target is latency to shed, not jitter headroom. */
  private readonly trimSlack: number;
  /** Audio that must arrive while above the slack before shedding (~2 s at a 100 ms target). */
  private readonly trimHold: number;
  /** Frames of underrun-free playback that relax the target by one step (~30 s). */
  private readonly relaxAfter: number;
  /** Frames over which `avgDepth` averages (~1 s): long enough to ignore burst arrival. */
  private readonly smoothOver: number;

  private target: number;
  private readPos = 0;
  private writePos = 0;
  private length = 0;
  private buffering = true;
  /** Fractional part of the read position; non-zero only while correcting drift. */
  private frac = 0;
  /** Low-passed depth, in frames — what the drift controller steers, never the raw depth. */
  private avgDepth: number;
  private cleanFrames = 0;
  private trimStreak = 0;
  /** Times playback has run dry since this buffer was built — the local half of audio loss. */
  underruns = 0;

  constructor(targetFrames: number, maxFrames: number, channels: number) {
    this.channels = channels;
    this.capacity = maxFrames;
    this.buf = new Float32Array(maxFrames * channels);
    const target = Math.min(targetFrames, maxFrames);
    this.target = target;
    this.floor = target;
    this.avgDepth = target;
    this.ceiling = Math.max(target, Math.min(3 * target, Math.floor(maxFrames / 2)));
    this.step = Math.max(1, Math.round(target / 2));
    this.trimSlack = 2 * target;
    this.trimHold = 20 * target;
    this.relaxAfter = 300 * target;
    this.smoothOver = 10 * target;
  }

  /** Buffered depth, in sample frames. */
  get buffered(): number {
    return this.length;
  }

  /** Current adaptive target depth, in sample frames. */
  get targetDepth(): number {
    return this.target;
  }

  /**
   * Append interleaved PCM. A push past capacity (tab sleep, network burst) sheds the oldest
   * frames all the way back to `target` — merely staying under the cap would park playback
   * ~maxFrames behind realtime for the rest of the stream. Sub-cap backlog is normally shed by
   * the drift controller instead; the streak below is the safety net for an excursion far too
   * large for ±0.4 % to walk off in reasonable time (a long concealed gap, say).
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
    if (this.avgDepth > this.target + this.trimSlack) {
      this.trimStreak += n;
      if (this.trimStreak >= this.trimHold) {
        this.dropOldest(this.length - this.target);
        this.avgDepth = this.target;
        this.trimStreak = 0;
      }
    } else {
      this.trimStreak = 0;
    }
  }

  /**
   * Fill one output buffer per channel with the next frames; silence while pre-buffering. An
   * underrun (fewer buffered frames than the outputs need) pads with silence, grows the target
   * and re-enters buffering until that target is reached again. Returns whether any real frames
   * were written.
   */
  read(outputs: Float32Array[]): boolean {
    const wanted = outputs[0]?.length ?? 0;
    if (wanted === 0) {
      return false;
    }
    if (this.buffering) {
      for (const out of outputs) {
        out.fill(0);
      }
      this.smooth(wanted);
      return false;
    }
    const cap = this.capacity;
    const ch = this.channels;
    const rate = this.driftRate();
    let pos = this.readPos;
    let frac = this.frac;
    let avail = this.length;
    let produced = 0;
    while (produced < wanted) {
      // Whole frames the fractional position has walked past are consumed first, so the
      // interpolation below always sits between `pos` and its successor.
      const whole = Math.floor(frac);
      if (whole > 0) {
        if (avail < whole) {
          break;
        }
        pos = (pos + whole) % cap;
        avail -= whole;
        frac -= whole;
      }
      // At rate 1 the position stays frame-aligned and no successor is needed: the common,
      // undrifted case stays a bit-exact copy rather than an interpolation of itself.
      if (avail < (frac > 0 ? 2 : 1)) {
        break;
      }
      const next = (pos + 1) % cap;
      for (let c = 0; c < outputs.length; c++) {
        const out = outputs[c];
        if (!out) {
          continue;
        }
        const lane = Math.min(c, ch - 1);
        const a = this.buf[pos * ch + lane] ?? 0;
        const b = this.buf[next * ch + lane] ?? 0;
        out[produced] = frac > 0 ? a + (b - a) * frac : a;
      }
      produced++;
      frac += rate;
    }
    // Frames the position has already walked past are consumed here rather than at the top of
    // the next call: until they are, `length` still counts audio that has been played, and the
    // capacity and trim arithmetic in `push` would be reading a depth one frame too deep.
    const played = Math.floor(frac);
    if (played > 0 && avail >= played) {
      pos = (pos + played) % cap;
      avail -= played;
      frac -= played;
    }
    this.readPos = pos;
    this.frac = frac;
    this.length = avail;
    if (produced < wanted) {
      for (const out of outputs) {
        out.fill(0, produced);
      }
      this.underrun();
    } else {
      this.cleanFrames += produced;
      if (this.cleanFrames >= this.relaxAfter) {
        this.cleanFrames = 0;
        this.target = Math.max(this.floor, this.target - this.step);
      }
    }
    this.smooth(wanted);
    return produced > 0;
  }

  clear(): void {
    this.readPos = 0;
    this.writePos = 0;
    this.length = 0;
    this.frac = 0;
    this.buffering = true;
    this.trimStreak = 0;
    this.cleanFrames = 0;
    // The learned target describes the path, not the stream, so it survives a restart; the
    // depth average must not, or the controller would steer against a depth from before.
    this.avgDepth = this.target;
  }

  /**
   * Playback rate that walks the smoothed depth back to target. Zero correction inside a 15 %
   * deadband (ordinary jitter is not drift), ramping to ±0.4 % — enough for any real clock pair
   * (tens of ppm) and far below the ~1 % where pitch starts to be audible.
   */
  private driftRate(): number {
    const deadband = 0.15;
    const maxDrift = 0.004;
    const error = (this.avgDepth - this.target) / this.target;
    const excess = Math.abs(error) - deadband;
    if (excess <= 0) {
      return 1;
    }
    const correction = Math.min(1, excess / (1 - deadband)) * maxDrift;
    return error > 0 ? 1 + correction : 1 - correction;
  }

  /** One-pole low pass on depth, weighted by how much audio the call consumed. */
  private smooth(frames: number): void {
    const alpha = Math.min(1, frames / this.smoothOver);
    this.avgDepth += (this.length - this.avgDepth) * alpha;
  }

  private underrun(): void {
    this.buffering = true;
    this.underruns += 1;
    this.cleanFrames = 0;
    this.trimStreak = 0;
    // The pre-buffer was too small for this path: hold more before resuming, so the next
    // hiccup of the same size is absorbed instead of heard.
    this.target = Math.min(this.ceiling, this.target + this.step);
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
