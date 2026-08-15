export class JitterBuffer {
  private readonly buf: Float32Array;
  private readonly channels: number;
  private readonly capacity: number;
  private readonly floor: number;
  private readonly ceiling: number;
  private readonly step: number;
  private readonly trimSlack: number;
  private readonly trimHold: number;
  private readonly relaxAfter: number;
  private readonly smoothOver: number;

  private target: number;
  private readPos = 0;
  private writePos = 0;
  private length = 0;
  private buffering = true;
  private frac = 0;
  private avgDepth: number;
  private cleanFrames = 0;
  private trimStreak = 0;
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

  get buffered(): number {
    return this.length;
  }

  get targetDepth(): number {
    return this.target;
  }

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
      const whole = Math.floor(frac);
      if (whole > 0) {
        if (avail < whole) {
          break;
        }
        pos = (pos + whole) % cap;
        avail -= whole;
        frac -= whole;
      }
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
    this.avgDepth = this.target;
  }

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

  private smooth(frames: number): void {
    const alpha = Math.min(1, frames / this.smoothOver);
    this.avgDepth += (this.length - this.avgDepth) * alpha;
  }

  private underrun(): void {
    this.buffering = true;
    this.underruns += 1;
    this.cleanFrames = 0;
    this.trimStreak = 0;
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
