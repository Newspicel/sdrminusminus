import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReactFlowProvider } from "@xyflow/react";
import type { ComponentType } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { type Workspace, WorkspaceProvider } from "../canvas/context";
import type { DeviceSet, PatchCatalog, PatchGraph, PatchNode } from "../lib/types";
import { SdrSocket } from "../lib/ws";
import { CATALOG } from "./catalog";

export interface FaceHarness {
  graph: PatchGraph;
  devices?: ReadonlyMap<string, DeviceSet>;
  catalog?: PatchCatalog;
  selected?: string | null;
}

const NO_DEVICES: ReadonlyMap<string, DeviceSet> = new Map();

export function stubWorkspace(harness: FaceHarness): Workspace {
  const devices = harness.devices ?? NO_DEVICES;
  return {
    socket: new SdrSocket(),
    graph: harness.graph,
    rack: {},
    settings: {},
    context: {
      catalog: harness.catalog ?? CATALOG,
      channelTypes: [],
      facets: [],
      bound: devices,
    },
    deviceSets: [...devices.values()],
    trunks: [],
    devices,
    channels: new Map(),
    owners: new Map(),
    savedChannels: new Map(),
    saveChannel: () => {},
    selected: harness.selected ?? null,
    select: () => {},
    expanded: null,
    expand: () => {},
    edit: () => {},
    editSettings: () => {},
    apply: () => {},
  };
}

export function renderFace(
  Face: ComponentType<{ node: PatchNode }>,
  node: PatchNode,
  harness: FaceHarness,
): string {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderToStaticMarkup(
    <QueryClientProvider client={queryClient}>
      <ReactFlowProvider>
        <WorkspaceProvider value={stubWorkspace(harness)}>
          <Face node={node} />
        </WorkspaceProvider>
      </ReactFlowProvider>
    </QueryClientProvider>,
  );
}
