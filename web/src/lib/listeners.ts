import type {
  AudioFrame,
  IqFrame,
  RangeDopplerFrame,
  SpectrumFrame,
  SymbolFrame,
  VideoFrame,
} from "./frame";
import type { ServerEvent } from "./types";

export interface SocketEvents {
  event: ServerEvent;
  status: boolean;
  audio: AudioFrame;
  spectrum: SpectrumFrame;
  iq: IqFrame;
  symbols: SymbolFrame;
  video: VideoFrame;
  surface: RangeDopplerFrame;
}

export type SocketEventKind = keyof SocketEvents;

export type Listener<K extends SocketEventKind> = (payload: SocketEvents[K]) => void;

export type Unsubscribe = () => void;

export class ListenerRegistry {
  private readonly listeners: { [K in SocketEventKind]: Set<Listener<K>> } = {
    event: new Set(),
    status: new Set(),
    audio: new Set(),
    spectrum: new Set(),
    iq: new Set(),
    symbols: new Set(),
    video: new Set(),
    surface: new Set(),
  };

  on<K extends SocketEventKind>(kind: K, listener: Listener<K>): Unsubscribe {
    const listeners = this.listeners[kind];
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }

  emit<K extends SocketEventKind>(kind: K, payload: SocketEvents[K]): void {
    for (const listener of this.listeners[kind]) {
      listener(payload);
    }
  }

  count(kind: SocketEventKind): number {
    return this.listeners[kind].size;
  }
}
