export type LossAction =
  | { kind: "continuous" }
  | { kind: "gap"; frames: number }
  | { kind: "reset"; frames: number };

export class LossTracker {
  private lastTimestamp: bigint | null = null;
  private learnedFrames: bigint | null = null;

  constructor(private readonly maxGapFrames: number) {}

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
