
export type LossAction =
  | { kind: "continuous" }
  | { kind: "gap"; frames: number }
  // Timestamps regressed or the hole is too big to conceal: drop buffered audio, rebuffer.
  // `frames` is how much was missing when that is knowable, 0 when the clock simply restarted.
  | { kind: "reset"; frames: number };

/**
 * Timestamps advance by exactly one packet's frames when nothing is lost. The packet
 * duration is not in the frame header, so it is learned from the deltas themselves: the
 * smallest observed delta is the duration, because loss makes deltas larger, never smaller.
 */
export class LossTracker {
  private lastTimestamp: bigint | null = null;
  private learnedFrames: bigint | null = null;

  constructor(private readonly maxGapFrames: number) {}

  /** The learned packet duration in frames, or null before two packets have been seen. */
  get packetFrames(): number | null {
    return this.learnedFrames === null ? null : Number(this.learnedFrames);
  }

  next(timestamp: bigint): LossAction {
    const last = this.lastTimestamp;
    this.lastTimestamp = timestamp;
    if (last === null) {
      return { kind: "continuous" };
    }
    const delta = timestamp - last;
    if (delta <= 0n) {
      // Non-monotonic clock ⇒ the stream restarted behind our back; relearn everything.
      this.learnedFrames = null;
      return { kind: "reset", frames: 0 };
    }
    if (this.learnedFrames === null || delta < this.learnedFrames) {
      this.learnedFrames = delta;
      return { kind: "continuous" };
    }
    const gap = delta - this.learnedFrames;
    if (gap === 0n) {
      return { kind: "continuous" };
    }
    if (gap > BigInt(this.maxGapFrames)) {
      return { kind: "reset", frames: Number(gap) };
    }
    return { kind: "gap", frames: Number(gap) };
  }

  reset(): void {
    this.lastTimestamp = null;
    this.learnedFrames = null;
  }
}
