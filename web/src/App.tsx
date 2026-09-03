import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ReactFlowProvider } from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAppHotkeys } from "./appHotkeys";
import { applyToasts } from "./canvas/applyToasts";
import { bindChannels, bindDevices, deviceNodeOf } from "./canvas/binding";
import { Canvas } from "./canvas/Canvas";
import { WorkspaceProvider } from "./canvas/context";
import { FullFace } from "./canvas/FullFace";
import { pruneRack } from "./canvas/graph";
import { Rack } from "./canvas/Rack";
import { useWorkspace } from "./canvas/useWorkspace";
import { type View, WorkspaceBar } from "./canvas/WorkspaceBar";
import { WorkspaceStart } from "./canvas/WorkspaceStart";
import { AboutPanel } from "./components/AboutPanel";
import { ServerDown } from "./components/ServerDown";
import { Shortcuts } from "./components/Shortcuts";
import { Toasts } from "./components/Toasts";
import { TokenGate } from "./components/TokenGate";
import { channelTypesQuery, patchCatalogQuery, stateQuery } from "./lib/api";
import { audioEngine } from "./lib/audio/useChannelAudio";
import { watchDevicePosition } from "./lib/position";
import { pushToast } from "./lib/toasts";
import type { PatchApplyReport, PatchGraph, WorkspaceSettings } from "./lib/types";
import { useChannelPatch } from "./lib/useChannelPatch";
import { useDevicePatch } from "./lib/useDevicePatch";
import { useSdrSocket } from "./lib/useSdrSocket";
import { ToolsDialog } from "./tools/ToolsDialog";

export function App() {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [view, setView] = useState<View>("patch");
  const [stepHz, setStepHz] = useState(100_000);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [showAbout, setShowAbout] = useState(false);
  const [openTool, setOpenTool] = useState<string | null>(null);

  const state = useQuery(stateQuery());
  const channelTypes = useQuery(channelTypesQuery());
  const catalog = useQuery(patchCatalogQuery());
  const workspace = useWorkspace();
  const { applyPatch, cachedSettings } = useDevicePatch();
  const { applyEdit } = useChannelPatch();
  const deviceSets = useMemo(() => state.data?.device_sets ?? [], [state.data?.device_sets]);
  const scanSession = state.data?.scan_session ?? null;
  const trunks = useMemo(() => state.data?.trunk_systems ?? [], [state.data?.trunk_systems]);

  const { socket, retrySocket } = useSdrSocket(queryClient, workspace.error);

  const snapshot = workspace.active?.snapshot ?? null;
  const graph: PatchGraph = useMemo(
    () => snapshot?.graph ?? { nodes: [], edges: [] },
    [snapshot?.graph],
  );
  const deviceGpsNodeKey = JSON.stringify(
    graph.nodes
      .filter((node) => node.kind === "gps" && node.data.source?.type === "device")
      .map((node) => node.id)
      .toSorted(),
  );
  const deviceGpsNodeIds = useMemo(
    () => JSON.parse(deviceGpsNodeKey) as string[],
    [deviceGpsNodeKey],
  );
  useEffect(() => {
    if (socket === null) {
      return;
    }
    return watchDevicePosition(socket, deviceGpsNodeIds);
    // oxlint-disable-next-line react/exhaustive-effect-dependencies -- a new revision can bind the same node to another radio
  }, [socket, deviceGpsNodeIds, workspace.active?.revision]);
  const announced = useRef<PatchApplyReport | null>(null);
  useEffect(() => {
    const report = workspace.applied;
    if (report === null || report === announced.current) {
      return;
    }
    announced.current = report;
    for (const message of applyToasts(report, graph.nodes)) {
      pushToast(message);
    }
  }, [workspace.applied, graph.nodes]);

  const rack = useMemo(() => pruneRack(snapshot?.rack ?? {}, graph), [snapshot?.rack, graph]);
  const settings = useMemo(() => snapshot?.settings ?? {}, [snapshot?.settings]);
  const save = workspace.save;
  const editSettings = useCallback(
    (change: Partial<WorkspaceSettings>) =>
      save((current) => ({
        ...current,
        settings: { ...current.settings, ...change },
      })),
    [save],
  );

  const devices = useMemo(() => bindDevices(graph, deviceSets), [graph, deviceSets]);
  const channels = useMemo(() => bindChannels(graph, devices), [graph, devices]);

  const reachable = useMemo(
    () =>
      [...devices.values()].flatMap((set) =>
        set.channels.map((channel) => ({ deviceSet: set.id, channel: channel.id })),
      ),
    [devices],
  );
  useEffect(() => {
    if (state.data !== undefined) {
      audioEngine.retain(reachable);
    }
  }, [reachable, state.data]);

  const context = useMemo(
    () => ({
      catalog: catalog.data ?? { nodes: [] },
      channelTypes: channelTypes.data?.types ?? [],
      bound: devices,
    }),
    [catalog.data, channelTypes.data, devices],
  );

  const selectedNode = graph.nodes.find((node) => node.id === selected) ?? null;
  const selectedChannel = selected === null ? null : (channels.get(selected) ?? null);
  const selectedDevice = selected === null ? null : deviceNodeOf(graph, selected);
  const selectedSet = selectedDevice === null ? null : (devices.get(selectedDevice) ?? null);

  const channelNodes = graph.nodes.filter((node) => node.kind === "channel");

  useAppHotkeys({
    selected,
    setSelected,
    selectedSet,
    selectedChannel,
    selectedNode,
    selectedDevice,
    channelNodes,
    graph,
    stepHz,
    setStepHz,
    workspace,
    applyPatch,
    cachedSettings,
    applyEdit,
    setView,
    setExpanded,
    setShowShortcuts,
  });

  return (
    <TokenGate onToken={() => socket?.retryNow()}>
      <div className="flex h-full flex-col bg-bg text-ink">
        {socket !== null && snapshot !== null && (
          <WorkspaceProvider
            value={{
              socket,
              graph,
              rack,
              settings,
              context,
              deviceSets,
              scanSession,
              trunks,
              devices,
              channels,
              selected,
              select: setSelected,
              expanded,
              expand: setExpanded,
              edit: workspace.save,
              editSettings,
              apply: workspace.apply,
            }}
          >
            <ReactFlowProvider>
              <WorkspaceBar
                view={view}
                onView={setView}
                workspaces={workspace.workspaces}
                activeWorkspace={workspace.active?.id ?? null}
                onActivate={workspace.activate}
                onCreate={workspace.create}
                onImport={workspace.importFile}
                onRemove={workspace.remove}
                onUndo={workspace.undo}
                onRedo={workspace.redo}
                canUndo={workspace.canUndo}
                canRedo={workspace.canRedo}
                onShowShortcuts={() => setShowShortcuts(true)}
                onOpenTool={setOpenTool}
              />
              <div className="relative flex min-h-0 flex-1 flex-col">
                {view === "patch" ? <Canvas /> : <Rack />}
                <FullFace />
              </div>
            </ReactFlowProvider>
          </WorkspaceProvider>
        )}

        {workspace.unreachable !== null && (
          <ServerDown reason={workspace.unreachable} onReachable={retrySocket} />
        )}

        {workspace.unreachable === null && workspace.active === null && !workspace.pending && (
          <WorkspaceStart onCreate={workspace.create} />
        )}

        <Shortcuts
          open={showShortcuts}
          onOpenChange={setShowShortcuts}
          onShowAbout={() => setShowAbout(true)}
        />
        <AboutPanel open={showAbout} onOpenChange={setShowAbout} />
        <ToolsDialog tool={openTool} onClose={() => setOpenTool(null)} />
        <Toasts />
      </div>
    </TokenGate>
  );
}
