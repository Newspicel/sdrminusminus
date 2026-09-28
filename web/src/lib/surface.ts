import type {
  FusionGridFrame,
  RangeDopplerFrame,
  SpatialSpectrumFrame,
  VisibilityFrame,
} from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import type { ClientCommand, ServerEvent, StreamKind, SurfaceFit } from "./types";

export type SurfaceFrame =
  | { kind: "range_doppler"; frame: RangeDopplerFrame }
  | { kind: "spatial_spectrum"; frame: SpatialSpectrumFrame }
  | { kind: "visibility"; frame: VisibilityFrame }
  | { kind: "fusion_grid"; frame: FusionGridFrame };

export type SurfaceKind = SurfaceFrame["kind"];

const SURFACE_KINDS: ReadonlySet<StreamKind> = new Set<SurfaceKind>([
  "range_doppler",
  "spatial_spectrum",
  "visibility",
  "fusion_grid",
]);

export function isSurfaceKind(kind: StreamKind): kind is SurfaceKind {
  return SURFACE_KINDS.has(kind);
}

export interface SurfaceSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  on<K extends "surface" | "status" | "event">(kind: K, listener: Listener<K>): Unsubscribe;
}

type SurfaceListener = (frame: SurfaceFrame) => void;

interface Watched {
  listeners: Set<SurfaceListener>;
  latest: SurfaceFrame | null;
  fit: SurfaceFit | undefined;
}

export class SurfaceHub {
  private socket: SurfaceSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];
  private readonly nodes = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (surface: SurfaceFrame): void => {
    const node = this.ids.get(surface.frame.streamId);
    const watched = node === undefined ? undefined : this.nodes.get(node);
    if (watched === undefined) {
      return;
    }
    watched.latest = surface;
    for (const listener of watched.listeners) {
      listener(surface);
    }
  };

  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "SurfaceStreamStarted") {
      this.ids.set(event.data.stream_id, event.data.node);
    } else if (event.type === "StreamStopped" && isSurfaceKind(event.data.kind)) {
      this.ids.delete(event.data.stream_id);
    }
  };

  private readonly onStatus = (connected: boolean): void => {
    if (connected) {
      this.resubscribe();
    }
  };

  attach(socket: SurfaceSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    this.unsubscribes = [
      socket.on("surface", this.onFrame),
      socket.on("status", this.onStatus),
      socket.on("event", this.onEvent),
    ];
    this.resubscribe();
  }

  detach(): void {
    this.socket = null;
    for (const unsubscribe of this.unsubscribes) {
      unsubscribe();
    }
    this.unsubscribes = [];
  }

  subscribe(node: string, listener: SurfaceListener, fit?: SurfaceFit): () => void {
    const watched = this.nodes.get(node);
    if (watched === undefined) {
      this.nodes.set(node, { listeners: new Set([listener]), latest: null, fit });
      this.send(node, true);
    } else {
      watched.listeners.add(listener);
    }
    return () => {
      const current = this.nodes.get(node);
      if (current === undefined) {
        return;
      }
      current.listeners.delete(listener);
      if (current.listeners.size === 0) {
        this.nodes.delete(node);
        this.send(node, false);
      }
    };
  }

  latest(node: string): SurfaceFrame | null {
    return this.nodes.get(node)?.latest ?? null;
  }

  watched(): string[] {
    return [...this.nodes.keys()];
  }

  private resubscribe(): void {
    this.ids.clear();
    for (const node of this.nodes.keys()) {
      this.send(node, true);
    }
  }

  private send(node: string, on: boolean): void {
    if (this.socket === null || !this.socket.isConnected()) {
      return;
    }
    if (!on) {
      this.socket.send({ type: "UnsubscribeSurface", data: { node } });
      return;
    }
    const fit = this.nodes.get(node)?.fit;
    this.socket.send({
      type: "SubscribeSurface",
      data: fit === undefined ? { node } : { node, fit },
    });
  }
}

export const surfaceHub = new SurfaceHub();
