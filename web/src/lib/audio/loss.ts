// Audio-loss detection (PLAN §9). Each Opus frame carries a 48 kHz-domain sample timestamp;
// a hole in that clock is the loss signal (seq is per-stream bookkeeping, not authoritative).
// Pure logic so the engine's gap handling stays unit-testable.

export type LossAction =
  | { kind: "continuous" }
  // `samples` of audio are missing; conceal them so buffered depth and timing stay honest.
  | { kind: "gap"; samples: number }
  // Timestamps regressed or the hole is too big to conceal: drop buffered audio, rebuffer.
  | { kind: "reset" };

/**
 * Timestamps advance by exactly one packet's samples when nothing is lost. The packet
 * duration is not in the frame header, so it is learned from the deltas themselves: the
 * smallest observed delta is the duration, because loss makes deltas larger, never smaller.
 */
export class LossTracker {
  private lastTimestamp: bigint | null = null;
  private packetSamples: bigint | null = null;

  constructor(private readonly maxGapSamples: number) {}

  next(timestamp: bigint): LossAction {
    const last = this.lastTimestamp;
    this.lastTimestamp = timestamp;
    if (last === null) {
      return { kind: "continuous" };
    }
    const delta = timestamp - last;
    if (delta <= 0n) {
      // Non-monotonic clock ⇒ the stream restarted behind our back; relearn everything.
      this.packetSamples = null;
      return { kind: "reset" };
    }
    if (this.packetSamples === null || delta < this.packetSamples) {
      this.packetSamples = delta;
      return { kind: "continuous" };
    }
    const gap = delta - this.packetSamples;
    if (gap === 0n) {
      return { kind: "continuous" };
    }
    if (gap > BigInt(this.maxGapSamples)) {
      return { kind: "reset" };
    }
    return { kind: "gap", samples: Number(gap) };
  }

  reset(): void {
    this.lastTimestamp = null;
    this.packetSamples = null;
  }
}
