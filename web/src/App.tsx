import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { ReactFlowProvider } from "@xyflow/react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { bindChannels, bindDevices, deviceNodeOf } from "./canvas/binding";
import { Canvas } from "./canvas/Canvas";
import { WorkspaceProvider } from "./canvas/context";
import { isPinned, patchNode, pin, pruneRack, unpin } from "./canvas/graph";
import { deviceDialId } from "./canvas/nodes/deviceNode";
import { Rack } from "./canvas/Rack";
import { useHotkeys } from "./canvas/useHotkeys";
import { useWorkspace } from "./canvas/useWorkspace";
import { type View, WorkspaceBar } from "./canvas/WorkspaceBar";
import { AboutPanel } from "./components/AboutPanel";
import { Button } from "./components/BaseControls";
import { BTN_PRIMARY } from "./components/controls";
import { TUNE_STEPS_HZ, tuningRange } from "./components/dial";
import { ServerDown } from "./components/ServerDown";
import { Shortcuts } from "./components/Shortcuts";
import { Toasts } from "./components/Toasts";
import { TokenGate } from "./components/TokenGate";
import {
  AUDIO_RECORDINGS_KEY,
  BOOKMARKS_KEY,
  CALLS_KEY,
  channelTypesQuery,
  DECODER_LOG_KEY,
  DEVICES_KEY,
  PRESETS_KEY,
  patchCatalogQuery,
  RECORDINGS_KEY,
  STATE_KEY,
  stateQuery,
  TEMPLATES_KEY,
  WORKSPACES_KEY,
} from "./lib/api";
import { audioEngine } from "./lib/audio/useChannelAudio";
import { useDecodedStore } from "./lib/decoded";
import { iqHub } from "./lib/iq";
import { useLevelStore } from "./lib/levels";
import { usePositionStore, watchDevicePosition } from "./lib/position";
import { useScannerStore } from "./lib/scanner";
import { spectrumHub } from "./lib/spectrum";
import { pushToast } from "./lib/toasts";
import type {
  PatchGraph,
  ServerEvent,
  StateScope,
  VoiceCall,
  VoiceCallsResponse,
  WorkspaceSettings,
} from "./lib/types";
import { useChannelPatch } from "./lib/useChannelPatch";
import { useDevicePatch } from "./lib/useDevicePatch";
import { videoHub } from "./lib/video";
import { SdrSocket } from "./lib/ws";
import { ToolsDialog } from "./tools/ToolsDialog";

const MODE_RING = ["nfm", "wfm", "am", "ssb"] as const;

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
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
  const trunks = useMemo(() => state.data?.trunk_systems ?? [], [state.data?.trunk_systems]);

  const onServerEvent = useCallback(
    (event: ServerEvent) => {
      switch (event.type) {
        case "Hello":
          void queryClient.invalidateQueries();
          break;
        case "StateChanged":
          invalidateScope(queryClient, event.data.scope);
          break;
        case "CallCompleted":
          appendCall(queryClient, event.data);
          break;
        case "Error":
          if (!audioEngine.claimServerError(event.data.message)) {
            pushToast(event.data.message);
          }
          break;
        default:
          break;
      }
    },
    [queryClient],
  );
  const onServerEventRef = useRef(onServerEvent);
  useLayoutEffect(() => {
    onServerEventRef.current = onServerEvent;
  });

  useEffect(() => {
    const s = new SdrSocket();
    s.onEvent = (event) => onServerEventRef.current(event);
    let up = false;
    s.onStatus = (now) => {
      if (up && !now) {
        pushToast("Lost the server — reconnecting");
      }
      up = now;
    };
    s.addEventListener(useDecodedStore.getState().observe);
    s.addEventListener(useScannerStore.getState().observe);
    s.addEventListener(usePositionStore.getState().observe);
    s.addEventListener(useLevelStore.getState().observe);
    spectrumHub.attach(s);
    iqHub.attach(s);
    videoHub.attach(s);
    audioEngine.attach(s);
    setSocket(s);
    s.connect();
    return () => {
      spectrumHub.detach();
      iqHub.detach();
      videoHub.detach();
      s.close();
    };
  }, []);

  useEffect(() => {
    if (workspace.error !== null) {
      pushToast(`Workspace: ${workspace.error}`);
    }
  }, [workspace.error]);

  const retrySocket = useCallback(() => socket?.retryNow(), [socket]);

  const snapshot = workspace.active?.snapshot ?? null;
  const graph: PatchGraph = useMemo(
    () => snapshot?.graph ?? { nodes: [], edges: [] },
    [snapshot?.graph],
  );
  const deviceGpsNodeKey = JSON.stringify(
    graph.nodes
      .filter((node) => node.kind === "gps" && (node.data.source?.type ?? "device") === "device")
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
  }, [socket, deviceGpsNodeIds, workspace.active?.revision]);
  useEffect(() => {
    for (const refusal of workspace.applied?.refused ?? []) {
      const node = graph.nodes.find((candidate) => candidate.id === refusal.node);
      const what =
        node?.label ?? (node?.kind === "channel" ? node.data.channel_type.toUpperCase() : "node");
      pushToast(`${what}: ${refusal.reason}`);
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

  useHotkeys({
    tune: (steps) => {
      if (selectedSet === null) {
        return;
      }
      const range = tuningRange(selectedSet.capabilities);
      const current = cachedSettings(selectedSet.id)?.center_hz ?? 0;
      const wanted = current + steps * stepHz;
      applyPatch(selectedSet.id, { center_hz: Math.min(range.max, Math.max(range.min, wanted)) });
    },
    stepBy: (direction) => {
      const at = TUNE_STEPS_HZ.indexOf(stepHz as (typeof TUNE_STEPS_HZ)[number]);
      const next = Math.min(TUNE_STEPS_HZ.length - 1, Math.max(0, at + direction));
      setStepHz(TUNE_STEPS_HZ[next] ?? stepHz);
    },
    focusDial: () => {
      if (selectedDevice !== null) {
        document.getElementById(deviceDialId(selectedDevice))?.focus();
      }
    },
    cycleMode: (direction) => {
      if (selectedSet === null || selectedChannel === null) {
        return;
      }
      const at = MODE_RING.indexOf(
        selectedChannel.settings.params.type as (typeof MODE_RING)[number],
      );
      const next = MODE_RING[(Math.max(0, at) + direction + MODE_RING.length) % MODE_RING.length];
      if (next === undefined || selected === null) {
        return;
      }
      applyEdit(selectedSet.id, selectedChannel.id, { params: { type: next, settings: {} } });
      workspace.save((current) => ({
        ...current,
        graph: patchNode(current.graph, selected, (node) =>
          node.kind === "channel"
            ? { ...node, kind: "channel" as const, data: { channel_type: next } }
            : node,
        ),
      }));
    },
    adjustSquelch: (deltaDb) => {
      if (selectedSet === null || selectedChannel === null) {
        return;
      }
      applyEdit(selectedSet.id, selectedChannel.id, (current) => ({
        squelch_db: Math.min(0, Math.max(-120, (current.squelch_db ?? -60) + deltaDb)),
      }));
    },
    toggleSquelch: () => {
      if (selectedSet === null || selectedChannel === null) {
        return;
      }
      applyEdit(selectedSet.id, selectedChannel.id, (current) => ({
        squelch_db: current.squelch_db == null ? -60 : null,
      }));
    },
    selectChannel: (direction) => {
      if (channelNodes.length === 0) {
        return;
      }
      const at = channelNodes.findIndex((node) => node.id === selected);
      const next = channelNodes[(at + direction + channelNodes.length) % channelNodes.length];
      setSelected(next?.id ?? null);
    },
    selectNode: (index) => setSelected(graph.nodes[index]?.id ?? null),
    togglePin: () => {
      if (selectedNode === null) {
        return;
      }
      workspace.save((current) => ({
        ...current,
        rack: isPinned(current.rack ?? {}, selectedNode.id)
          ? unpin(current.rack ?? {}, selectedNode.id)
          : pin(current.rack ?? {}, selectedNode.id),
      }));
    },
    toggleView: () => setView((current) => (current === "patch" ? "rack" : "patch")),
    undo: workspace.undo,
    redo: workspace.redo,
    showShortcuts: () => setShowShortcuts(true),
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
              trunks,
              devices,
              channels,
              selected,
              select: setSelected,
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
                onRemove={workspace.remove}
                onUndo={workspace.undo}
                onRedo={workspace.redo}
                canUndo={workspace.canUndo}
                canRedo={workspace.canRedo}
                onShowShortcuts={() => setShowShortcuts(true)}
                onOpenTool={setOpenTool}
              />
              {view === "patch" ? <Canvas /> : <Rack />}
            </ReactFlowProvider>
          </WorkspaceProvider>
        )}

        {workspace.unreachable !== null && (
          <ServerDown reason={workspace.unreachable} onReachable={retrySocket} />
        )}

        {workspace.unreachable === null && workspace.active === null && !workspace.pending && (
          <div className="flex min-h-0 flex-1 items-center justify-center">
            <Button
              type="button"
              className={BTN_PRIMARY}
              onClick={() => workspace.create("Workspace")}
            >
              Create a workspace
            </Button>
          </div>
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

const MAX_CACHED_CALLS = 10_000;

function appendCall(queryClient: QueryClient, call: VoiceCall) {
  queryClient.setQueryData(CALLS_KEY, (previous: VoiceCallsResponse | undefined) => ({
    calls: [call, ...(previous?.calls ?? [])].slice(0, MAX_CACHED_CALLS),
  }));
}

function invalidateScope(queryClient: QueryClient, scope: StateScope): void {
  switch (scope.scope) {
    case "all":
      void queryClient.invalidateQueries();
      break;
    case "devices":
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      void queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
      void queryClient.invalidateQueries({ queryKey: TEMPLATES_KEY });
      break;
    case "device_set":
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      break;
    case "presets":
      void queryClient.invalidateQueries({ queryKey: PRESETS_KEY });
      break;
    case "bookmarks":
      void queryClient.invalidateQueries({ queryKey: BOOKMARKS_KEY });
      break;
    case "recordings":
      void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
      void queryClient.invalidateQueries({ queryKey: AUDIO_RECORDINGS_KEY });
      break;
    case "workspaces":
      void queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY });
      break;
    case "decoder_log":
      void queryClient.invalidateQueries({ queryKey: DECODER_LOG_KEY });
      break;
    case "calls":
      void queryClient.invalidateQueries({ queryKey: CALLS_KEY });
      break;
  }
}
