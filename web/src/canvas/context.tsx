import { createContext, type ReactNode, useContext } from "react";
import type {
  ChannelInfo,
  DeviceSet,
  PatchGraph,
  RackLayout,
  ScanSession,
  TrunkSystemStatus,
  WorkspaceSettings,
  WorkspaceSnapshot,
} from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import type { GraphContext } from "./graph";

export interface Workspace {
  socket: SdrSocket;
  graph: PatchGraph;
  rack: RackLayout;
  settings: WorkspaceSettings;
  context: GraphContext;
  deviceSets: readonly DeviceSet[];
  scanSession: ScanSession | null;
  trunks: readonly TrunkSystemStatus[];
  devices: ReadonlyMap<string, DeviceSet>;
  channels: ReadonlyMap<string, ChannelInfo>;
  selected: string | null;
  select: (node: string | null) => void;
  edit: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  editSettings: (settings: Partial<WorkspaceSettings>) => void;
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
