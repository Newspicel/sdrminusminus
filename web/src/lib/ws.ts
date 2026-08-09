// One WebSocket per client (PLAN §5): JSON `ServerEvent`s + binary spectrum frames in, JSON
// `ClientCommand`s out. Auto-reconnects; the caller sets the handler fields.
import { decodeSpectrum, type SpectrumFrame } from "./frame";
import type { ClientCommand, ServerEvent } from "./types";

const RECONNECT_MS = 1000;

export class SdrSocket {
  private ws: WebSocket | null = null;
  private reconnectTimer: number | null = null;
  private closed = false;
  private readonly url: string;

  onEvent: (event: ServerEvent) => void = () => {};
  onSpectrum: (frame: SpectrumFrame) => void = () => {};
  onStatus: (connected: boolean) => void = () => {};

  constructor(path = "/api/ws") {
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    this.url = `${proto}//${window.location.host}${path}`;
  }

  connect(): void {
    this.closed = false;
    this.open();
  }

  send(command: ClientCommand): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(command));
    }
  }

  close(): void {
    this.closed = true;
    if (this.reconnectTimer !== null) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.ws?.close();
    this.ws = null;
  }

  private open(): void {
    const ws = new WebSocket(this.url);
    ws.binaryType = "arraybuffer";
    ws.onopen = () => this.onStatus(true);
    ws.onerror = () => ws.close();
    ws.onclose = () => {
      this.onStatus(false);
      this.scheduleReconnect();
    };
    ws.onmessage = (event: MessageEvent<string | ArrayBuffer>) => {
      if (typeof event.data === "string") {
        this.dispatchText(event.data);
      } else {
        const frame = decodeSpectrum(event.data);
        if (frame) {
          this.onSpectrum(frame);
        }
      }
    };
    this.ws = ws;
  }

  private dispatchText(text: string): void {
    try {
      this.onEvent(JSON.parse(text) as ServerEvent);
    } catch {
      // Ignore malformed frames; the server is the source of truth and will resend state.
    }
  }

  private scheduleReconnect(): void {
    if (this.closed || this.reconnectTimer !== null) {
      return;
    }
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null;
      if (!this.closed) {
        this.open();
      }
    }, RECONNECT_MS);
  }
}
