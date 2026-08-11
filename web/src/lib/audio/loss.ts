// Audio-loss detection (PLAN §9). Each Opus frame carries a 48 kHz-domain sample-frame
// timestamp; a hole in that clock is the loss signal (seq is per-stream bookkeeping, not
// authoritative). Counting frames rather than samples is what keeps this independent of the
// stream's channel layout, which can change mid-stream. Pure logic so the engine's gap
// handling stays unit-testable.

export type LossAction =
  | { kind: "continuous" }
  // `frames` of audio are missing; conceal them so buffered depth and timing stay honest.
  | { kind: "gap"; frames: number }
  // Timestamps regressed or the hole is too big to conceal: drop buffered audio, rebuffer.
  | { kind: "reset" };

/**
 * Timestamps advance by exactly one packet's frames when nothing is lost. The packet
 * duration is not in the frame header, so it is learned from the deltas themselves: the
 * smallest observed delta is the duration, because loss makes deltas larger, never smaller.
 */
export class LossTracker {
  private lastTimestamp: bigint | null = null;
  private packetFrames: bigint | null = null;

  constructor(private readonly maxGapFrames: number) {}

  next(timestamp: bigint): LossAction {
    const last = this.lastTimestamp;
    this.lastTimestamp = timestamp;
    if (last === null) {
      return { kind: "continuous" };
    }
    const delta = timestamp - last;
    if (delta <= 0n) {
      // Non-monotonic clock ⇒ the stream restarted behind our back; relearn everything.
      this.packetFrames = null;
      return { kind: "reset" };
    }
    if (this.packetFrames === null || delta < this.packetFrames) {
      this.packetFrames = delta;
      return { kind: "continuous" };
    }
    const gap = delta - this.packetFrames;
    if (gap === 0n) {
      return { kind: "continuous" };
    }
    if (gap > BigInt(this.maxGapFrames)) {
      return { kind: "reset" };
    }
    return { kind: "gap", frames: Number(gap) };
  }

  reset(): void {
    this.lastTimestamp = null;
    this.packetFrames = null;
  }
}
