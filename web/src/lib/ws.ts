
import { withToken } from "./auth";
import {
  type AudioFrame,
  decodeAudio,
  decodeSpectrum,
  decodeVideo,
  FRAME_KIND_AUDIO_OPUS,
  FRAME_KIND_SPECTRUM,
  FRAME_KIND_VIDEO_GRAY,
  frameKind,
  type SpectrumFrame,
  type VideoFrame,
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
  private readonly spectrumListeners = new Set<(frame: SpectrumFrame) => void>();
  private readonly videoListeners = new Set<(frame: VideoFrame) => void>();

  onEvent: (event: ServerEvent) => void = () => {};
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

  addSpectrumListener(listener: (frame: SpectrumFrame) => void): void {
    this.spectrumListeners.add(listener);
  }

  removeSpectrumListener(listener: (frame: SpectrumFrame) => void): void {
    this.spectrumListeners.delete(listener);
  }

  addVideoListener(listener: (frame: VideoFrame) => void): void {
    this.videoListeners.add(listener);
  }

  removeVideoListener(listener: (frame: VideoFrame) => void): void {
    this.videoListeners.delete(listener);
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
    this.detach();
  }

  /** Silence and close the current socket, so nothing it does afterwards reaches this
   * instance's handlers. */
  private detach(): void {
    const ws = this.ws;
    this.ws = null;
    if (ws === null) {
      return;
    }
    ws.onopen = null;
    ws.onerror = null;
    ws.onclose = null;
    ws.onmessage = null;
    ws.close();
  }

  private open(): void {
    // Detach the previous socket first: a close that arrives after its replacement exists
    // would otherwise schedule a *second* reconnect, and both sockets would fan duplicate
    // frames into the same handlers.
    this.detach();
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
          for (const listener of this.spectrumListeners) {
            listener(frame);
          }
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
      case FRAME_KIND_VIDEO_GRAY: {
        const frame = decodeVideo(buffer);
        if (frame) {
          for (const listener of this.videoListeners) {
            listener(frame);
          }
        }
        break;
      }
      default:
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
      this.open();
    }
  }
}
