const REFRESH_MS = 250;
const SETTLE_MS = 400;

export class ReadoutHold {
  private bin: number | null = null;
  private power = 0;
  private settled = 0;
  private refreshed = 0;
  private shown = Number.NEGATIVE_INFINITY;

  read(bin: number, db: number, now: number): number {
    if (!Number.isFinite(db)) {
      this.bin = null;
      return db;
    }
    const level = 10 ** (db / 10);
    if (bin !== this.bin) {
      this.bin = bin;
      this.power = level;
      this.settled = now;
      this.refreshed = now;
      this.shown = db;
      return db;
    }
    const weight = 1 - Math.exp(-Math.max(0, now - this.settled) / SETTLE_MS);
    this.power += (level - this.power) * weight;
    this.settled = now;
    if (now - this.refreshed >= REFRESH_MS) {
      this.refreshed = now;
      this.shown = 10 * Math.log10(this.power + 1e-30);
    }
    return this.shown;
  }
}
