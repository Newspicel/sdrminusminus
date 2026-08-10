// What every node face needs and cannot receive as a prop: React Flow keeps node data in its
// own store, so a socket, a `DeviceSet` or a setter must not travel that way (the same reason
// the M6 dock had a context). Faces read this; only ids ever reach the stored patch.
import { createContext, type ReactNode, useContext } from "react";
import type {
  ChannelInfo,
  DeviceSet,
  PatchGraph,
  RackLayout,
  WorkspaceSnapshot,
} from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import { deviceNodeOf } from "./binding";
import type { GraphContext } from "./graph";

export interface Station {
  socket: SdrSocket;
  connected: boolean;
  /** The patch as stored. Faces read their own node out of it; the canvas owns the geometry. */
  graph: PatchGraph;
  rack: RackLayout;
  /** Ports and channel types — the generated tables the drag-time rules are built from. */
  context: GraphContext;
  /** Every open device set, for the pickers that have to name a radio. */
  deviceSets: readonly DeviceSet[];
  /** Device node id → the set it drives right now (CANVAS §3). Absent means disconnected. */
  devices: ReadonlyMap<string, DeviceSet>;
  /** Channel node id → the engine channel it drives. Absent means apply has not created it. */
  channels: ReadonlyMap<string, ChannelInfo>;
  /** Selected node — the one the keyboard acts on. */
  selected: string | null;
  select: (node: string | null) => void;
  /** Edit the stored patch. Debounced and revision-checked by the station hook. */
  edit: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  /** Ask the server to bring the engine up to the patch (open radios, add channels). */
  apply: () => void;
}

const StationContext = createContext<Station | null>(null);

export function StationProvider({ value, children }: { value: Station; children: ReactNode }) {
  return <StationContext value={value}>{children}</StationContext>;
}

export function useStationContext(): Station {
  const station = useContext(StationContext);
  if (station === null) {
    throw new Error("useStationContext outside StationProvider");
  }
  return station;
}

/** The device set behind a node, following the wire when the node is a channel, a sink or a
 * scanner (`deviceNodeOf`). Returns `null` while the radio is absent — the face renders
 * disconnected rather than empty. */
export function deviceSetOf(station: Station, node: string): DeviceSet | null {
  const owner = deviceNodeOf(station.graph, node);
  return owner === null ? null : (station.devices.get(owner) ?? null);
}
