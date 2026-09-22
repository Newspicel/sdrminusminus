import {
  type DemoSession,
  isUnsubscribe,
  type RecordedMessage,
  subscriptionKey,
} from "../../../web/e2e/demoSession";
import { due, type FrameTrack, frameTrack, lapped } from "./timeline";

interface Stream {
  started: string;
  track: FrameTrack;
}

export type Emit = (data: string | ArrayBuffer) => void;

export class Player {
  private readonly streams = new Map<string, Stream>();
  private readonly active = new Set<string>();
  private readonly events: RecordedMessage[];
  private readonly loopMs: number;
  private readonly greeting: string[];
  private startedAt = 0;
  private played = 0;

  constructor(
    session: DemoSession,
    private readonly emit: Emit,
    private readonly now: () => number,
  ) {
    for (const stream of session.streams) {
      this.streams.set(stream.key, { started: stream.started, track: frameTrack(stream.frames) });
    }
    this.events = session.events.filter((event) => event.text !== undefined);
    this.loopMs = session.loopMs;
    this.greeting = session.greeting;
  }

  open(): void {
    this.startedAt = this.now();
    this.played = 0;
    for (const text of this.greeting) {
      this.emit(text);
    }
  }

  command(text: string): void {
    const command = JSON.parse(text) as { type: string; data?: Record<string, unknown> };
    const key = subscriptionKey(command);
    const stream = key === null ? undefined : this.streams.get(key);
    if (key === null || stream === undefined) {
      return;
    }
    if (isUnsubscribe(command)) {
      this.active.delete(key);
      return;
    }
    this.active.add(key);
    this.emit(stream.started);
  }

  tick(): void {
    const elapsed = this.now() - this.startedAt;
    for (const { item } of due(this.events, this.loopMs, this.played, elapsed)) {
      this.emit(item.text ?? "");
    }
    for (const key of this.active) {
      const track = this.streams.get(key)?.track;
      if (track === undefined) {
        continue;
      }
      for (const { item, lap } of due(track.frames, this.loopMs, this.played, elapsed)) {
        this.emit(lapped(track, item, lap));
      }
    }
    this.played = elapsed;
  }
}
