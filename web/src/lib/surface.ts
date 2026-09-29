import labels from "../generated/labels.json";
import type {
  FusionGridFrame,
  RangeDopplerFrame,
  SpatialSpectrumFrame,
  VisibilityFrame,
} from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import type { ClientCommand, ServerEvent, StreamKind, SurfaceFit, SurfaceRefusal } from "./types";

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

const REFUSAL_TEXT: Readonly<Record<SurfaceRefusal, string>> = labels.surface_refusal;

export function refusalText(reason: SurfaceRefusal): string {
  return REFUSAL_TEXT[reason];
}

export class SurfaceHub {
  private socket: SurfaceSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];
  private readonly nodes = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();
  private readonly awaiting = new Set<string>();
  private readonly refusals = new Map<string, SurfaceRefusal>();
  private readonly refusalListeners = new Set<() => void>();

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
      this.awaiting.delete(event.data.node);
      this.setRefusal(event.data.node, null);
    } else if (event.type === "SurfaceRefused") {
      this.awaiting.delete(event.data.node);
      if (this.nodes.has(event.data.node)) {
        this.setRefusal(event.data.node, event.data.reason);
      }
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
      if (watched.latest !== null) {
        listener(watched.latest);
      }
    }
    return () => {
      const current = this.nodes.get(node);
      if (current === undefined) {
        return;
      }
      current.listeners.delete(listener);
      if (current.listeners.size === 0) {
        this.nodes.delete(node);
        this.setRefusal(node, null);
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

  refusal(node: string): SurfaceRefusal | null {
    return this.refusals.get(node) ?? null;
  }

  watchRefusals(listener: () => void): () => void {
    this.refusalListeners.add(listener);
    return () => {
      this.refusalListeners.delete(listener);
    };
  }

  retry(): void {
    const started = new Set(this.ids.values());
    for (const node of this.nodes.keys()) {
      if (!started.has(node) && !this.awaiting.has(node)) {
        this.send(node, true);
      }
    }
  }

  private setRefusal(node: string, reason: SurfaceRefusal | null): void {
    if ((this.refusals.get(node) ?? null) === reason) {
      return;
    }
    if (reason === null) {
      this.refusals.delete(node);
    } else {
      this.refusals.set(node, reason);
    }
    for (const listener of this.refusalListeners) {
      listener();
    }
  }

  private resubscribe(): void {
    this.ids.clear();
    this.awaiting.clear();
    for (const node of this.nodes.keys()) {
      this.send(node, true);
    }
  }

  private send(node: string, on: boolean): void {
    if (this.socket === null || !this.socket.isConnected()) {
      return;
    }
    if (!on) {
      this.awaiting.delete(node);
      this.socket.send({ type: "UnsubscribeSurface", data: { node } });
      return;
    }
    this.awaiting.add(node);
    const fit = this.nodes.get(node)?.fit;
    this.socket.send({
      type: "SubscribeSurface",
      data: fit === undefined ? { node } : { node, fit },
    });
  }
}

export const surfaceHub = new SurfaceHub();
