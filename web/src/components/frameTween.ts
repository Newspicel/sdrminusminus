const FIRST_INTERVAL_MS = 1000 / 30;
const MIN_INTERVAL_MS = 5;
const MAX_INTERVAL_MS = 200;
const INTERVAL_GLIDE = 0.2;

export class FrameTween {
  private from = new Float32Array(0);
  private to = new Float32Array(0);
  private shown = new Float32Array(0);
  private arrived = 0;
  private interval = FIRST_INTERVAL_MS;

  push(db: Float32Array, now: number): void {
    if (db.length !== this.to.length) {
      this.jump(db, now);
      return;
    }
    this.sample(now);
    this.from.set(this.shown);
    this.to.set(db);
    const gap = Math.min(MAX_INTERVAL_MS, Math.max(MIN_INTERVAL_MS, now - this.arrived));
    this.interval += (gap - this.interval) * INTERVAL_GLIDE;
    this.arrived = now;
  }

  jump(db: Float32Array, now: number): void {
    if (db.length !== this.to.length) {
      this.from = new Float32Array(db.length);
      this.to = new Float32Array(db.length);
      this.shown = new Float32Array(db.length);
    }
    this.from.set(db);
    this.to.set(db);
    this.shown.set(db);
    this.arrived = now;
  }

  sample(now: number): Float32Array {
    const t = Math.min(1, Math.max(0, (now - this.arrived) / this.interval));
    const { from, to, shown } = this;
    for (let i = 0; i < shown.length; i++) {
      const a = from[i] ?? 0;
      shown[i] = a + ((to[i] ?? 0) - a) * t;
    }
    return shown;
  }
}
