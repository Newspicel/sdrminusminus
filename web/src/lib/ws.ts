// One WebSocket per client (PLAN §5): JSON `ServerEvent`s + binary frames in, JSON
// `ClientCommand`s out. Auto-reconnects. The app shell owns the single-handler fields
// (`onEvent`/`onSpectrum`/`onStatus`/`onAudio`); subsystems that must observe the same
// events without stealing those use the add/remove listener methods.

import { withToken } from "./auth";
import {
  type AudioFrame,
  decodeAudio,
  decodeSpectrum,
  FRAME_KIND_AUDIO_OPUS,
  FRAME_KIND_SPECTRUM,
  frameKind,
  type SpectrumFrame,
} from "./frame";
import type { ClientCommand, ServerEvent } from "./types";

/** First reconnect delay; each further failure doubles it up to [`RECONNECT_MAX_MS`]. A fixed
 * 1 s retry turned a stopped server — or a wrong token, which the browser reports as a plain
 * close — into a 1 Hz request flood for as long as the tab stayed open. */
const RECONNECT_MS = 1000;
const RECONNECT_MAX_MS = 30_000;

export class SdrSocket {
  private ws: WebSocket | null = null;
  private reconnectTimer: number | null = null;
  private closed = false;
  private backoffMs = RECONNECT_MS;
  private readonly path: string;
  private readonly eventListeners = new Set<(event: ServerEvent) => void>();
  private readonly statusListeners = new Set<(connected: boolean) => void>();

  onEvent: (event: ServerEvent) => void = () => {};
  onSpectrum: (frame: SpectrumFrame) => void = () => {};
  onStatus: (connected: boolean) => void = () => {};
  onAudio: (frame: AudioFrame) => void = () => {};

  constructor(path = "/api/ws") {
    this.path = path;
  }

  /** Built per connection, not once: the token can be entered after the socket exists, and the
   * browser WebSocket API has no way to send it as a header. */
  private url(): string {
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    return withToken(`${proto}//${window.location.host}${this.path}`);
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

  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  addEventListener(listener: (event: ServerEvent) => void): void {
    this.eventListeners.add(listener);
  }

  removeEventListener(listener: (event: ServerEvent) => void): void {
    this.eventListeners.delete(listener);
  }

  addStatusListener(listener: (connected: boolean) => void): void {
    this.statusListeners.add(listener);
  }

  removeStatusListener(listener: (connected: boolean) => void): void {
    this.statusListeners.delete(listener);
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
    const ws = new WebSocket(this.url());
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      this.backoffMs = RECONNECT_MS;
      this.emitStatus(true);
    };
    ws.onerror = () => ws.close();
    ws.onclose = () => {
      this.emitStatus(false);
      this.scheduleReconnect();
    };
    ws.onmessage = (event: MessageEvent<string | ArrayBuffer>) => {
      if (typeof event.data === "string") {
        this.dispatchText(event.data);
      } else {
        this.dispatchBinary(event.data);
      }
    };
    this.ws = ws;
  }

  private dispatchText(text: string): void {
    let event: ServerEvent;
    try {
      event = JSON.parse(text) as ServerEvent;
    } catch {
      // Ignore malformed frames; the server is the source of truth and will resend state.
      return;
    }
    this.onEvent(event);
    for (const listener of this.eventListeners) {
      listener(event);
    }
  }

  private dispatchBinary(buffer: ArrayBuffer): void {
    switch (frameKind(buffer)) {
      case FRAME_KIND_SPECTRUM: {
        const frame = decodeSpectrum(buffer);
        if (frame) {
          this.onSpectrum(frame);
        }
        break;
      }
      case FRAME_KIND_AUDIO_OPUS: {
        const frame = decodeAudio(buffer);
        if (frame) {
          this.onAudio(frame);
        }
        break;
      }
      default:
        // Unknown kinds (e.g. future IQ_F32) are ignorable by design (PLAN §5).
        break;
    }
  }

  private emitStatus(connected: boolean): void {
    this.onStatus(connected);
    for (const listener of this.statusListeners) {
      listener(connected);
    }
  }

  private scheduleReconnect(): void {
    if (this.closed || this.reconnectTimer !== null) {
      return;
    }
    const delay = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, RECONNECT_MAX_MS);
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null;
      if (!this.closed) {
        this.open();
      }
    }, delay);
  }

  /** Reconnect now, resetting the backoff — for when the reason it was failing is known to be
   * fixed (a token was just entered). */
  retryNow(): void {
    if (this.reconnectTimer !== null) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.backoffMs = RECONNECT_MS;
    if (!this.closed && this.ws?.readyState !== WebSocket.OPEN) {
      this.ws?.close();
      this.open();
    }
  }
}
