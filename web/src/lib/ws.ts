import { withToken } from "./auth";
import {
  decodeAudio,
  decodeIq,
  decodeSpectrum,
  decodeVideo,
  FRAME_KIND_AUDIO_OPUS,
  FRAME_KIND_IQ_F32,
  FRAME_KIND_SPECTRUM,
  FRAME_KIND_VIDEO_GRAY,
  FRAME_KIND_VIDEO_RGB,
  frameKind,
} from "./frame";
import {
  type Listener,
  ListenerRegistry,
  type SocketEventKind,
  type SocketEvents,
  type Unsubscribe,
} from "./listeners";
import type { ClientCommand, ServerEvent } from "./types";

const RECONNECT_MS = 1000;
const RECONNECT_MAX_MS = 30_000;

export class SdrSocket {
  private ws: WebSocket | null = null;
  private reconnectTimer: number | null = null;
  private closed = false;
  private backoffMs = RECONNECT_MS;
  private readonly path: string;
  private readonly listeners = new ListenerRegistry();

  constructor(path = "/api/ws") {
    this.path = path;
  }

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

  on<K extends SocketEventKind>(kind: K, listener: Listener<K>): Unsubscribe {
    return this.listeners.on(kind, listener);
  }

  close(): void {
    this.closed = true;
    if (this.reconnectTimer !== null) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.detach();
  }

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
    this.detach();
    const ws = new WebSocket(this.url());
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      this.backoffMs = RECONNECT_MS;
      this.listeners.emit("status", true);
    };
    ws.onerror = () => ws.close();
    ws.onclose = () => {
      this.listeners.emit("status", false);
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
    this.listeners.emit("event", event);
  }

  private dispatchBinary(buffer: ArrayBuffer): void {
    switch (frameKind(buffer)) {
      case FRAME_KIND_SPECTRUM:
        this.emitFrame("spectrum", decodeSpectrum(buffer));
        break;
      case FRAME_KIND_AUDIO_OPUS:
        this.emitFrame("audio", decodeAudio(buffer));
        break;
      case FRAME_KIND_IQ_F32:
        this.emitFrame("iq", decodeIq(buffer));
        break;
      case FRAME_KIND_VIDEO_GRAY:
      case FRAME_KIND_VIDEO_RGB:
        this.emitFrame("video", decodeVideo(buffer));
        break;
      default:
        break;
    }
  }

  private emitFrame<K extends SocketEventKind>(kind: K, frame: SocketEvents[K] | null): void {
    if (frame !== null) {
      this.listeners.emit(kind, frame);
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
