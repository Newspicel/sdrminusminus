// What every node face needs and cannot receive as a prop: React Flow keeps node data in its
// own store, so a socket, a `DeviceSet` or a setter must not travel that way (the same reason
// the M6 dock had a context). Faces read this; only ids ever reach the stored patch.
import { createContext, type ReactNode, useContext } from "react";
import type {
  ChannelInfo,
  DeviceSet,
  PatchGraph,
  RackLayout,
  TrunkSystemStatus,
  WorkspaceSettings,
  WorkspaceSnapshot,
} from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import { deviceNodeOf } from "./binding";
import type { GraphContext } from "./graph";

export interface Workspace {
  socket: SdrSocket;
  /** The patch as stored. Faces read their own node out of it; the canvas owns the geometry. */
  graph: PatchGraph;
  rack: RackLayout;
  settings: WorkspaceSettings;
  /** Ports and channel types — the generated tables the drag-time rules are built from. */
  context: GraphContext;
  /** Every open device set, for the pickers that have to name a radio. */
  deviceSets: readonly DeviceSet[];
  /** What each trunk node is following. Its traffic channels have no node of their own. */
  trunks: readonly TrunkSystemStatus[];
  devices: ReadonlyMap<string, DeviceSet>;
  /** Channel node id → the engine channel it drives. Absent means apply has not created it. */
  channels: ReadonlyMap<string, ChannelInfo>;
  /** Selected node — the one the keyboard acts on. */
  selected: string | null;
  select: (node: string | null) => void;
  /** Edit the stored patch. Debounced and revision-checked by the workspace hook. */
  edit: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  /** Change some of the workspace's settings, leaving the rest as they are. */
  editSettings: (settings: Partial<WorkspaceSettings>) => void;
  /** Ask the server to bring the engine up to the patch (open radios, add channels). */
  apply: () => void;
}

const WorkspaceContext = createContext<Workspace | null>(null);

export function WorkspaceProvider({ value, children }: { value: Workspace; children: ReactNode }) {
  return <WorkspaceContext value={value}>{children}</WorkspaceContext>;
}

export function useWorkspaceContext(): Workspace {
  const workspace = useContext(WorkspaceContext);
  if (workspace === null) {
    throw new Error("useWorkspaceContext outside WorkspaceProvider");
  }
  return workspace;
}

/** The device set behind a node, following the wire when the node is a channel, a sink or a
 * scanner (`deviceNodeOf`). Returns `null` while the radio is absent — the face renders
 * disconnected rather than empty. */
export function deviceSetOf(workspace: Workspace, node: string): DeviceSet | null {
  const owner = deviceNodeOf(workspace.graph, node);
  return owner === null ? null : (workspace.devices.get(owner) ?? null);
}
