import type { DemoSession } from "../../../web/e2e/demoSession";
import { Player } from "./player";

const TICK_MS = 20;

export function demoSocketClass(session: Promise<DemoSession>) {
  return class DemoSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;

    readonly url: string;
    readyState = DemoSocket.CONNECTING;
    binaryType: BinaryType = "blob";
    onopen: ((event: Event) => void) | null = null;
    onmessage: ((event: MessageEvent) => void) | null = null;
    onclose: ((event: CloseEvent) => void) | null = null;
    onerror: ((event: Event) => void) | null = null;
    private player: Player | null = null;
    private timer: ReturnType<typeof setInterval> | null = null;

    constructor(url: string | URL) {
      this.url = String(url);
      void session.then((loaded) => this.start(loaded));
    }

    send(data: string): void {
      if (this.readyState === DemoSocket.OPEN) {
        this.player?.command(data);
      }
    }

    close(): void {
      if (this.timer !== null) {
        clearInterval(this.timer);
        this.timer = null;
      }
      if (this.readyState === DemoSocket.CLOSED) {
        return;
      }
      this.readyState = DemoSocket.CLOSED;
      this.onclose?.(new CloseEvent("close", { wasClean: true, code: 1000 }));
    }

    private start(loaded: DemoSession): void {
      if (this.readyState !== DemoSocket.CONNECTING) {
        return;
      }
      const player = new Player(
        loaded,
        (data) => this.onmessage?.(new MessageEvent("message", { data })),
        () => performance.now(),
      );
      this.player = player;
      this.readyState = DemoSocket.OPEN;
      this.onopen?.(new Event("open"));
      player.open();
      this.timer = setInterval(() => player.tick(), TICK_MS);
    }
  };
}
