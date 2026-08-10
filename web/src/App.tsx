// App shell (PLAN §10, CANVAS §1). Owns the WebSocket, turns `StateChanged` events into
// TanStack Query invalidations (the only invalidation path — no polling), and frames the
// station in one row of chrome above the patch or the rack.
//
// There is no device bar and no tab bar any more: identity is spatial (PLAN §18). Which radio
// you are operating is the node you are looking at, and the wires leaving it.
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { ReactFlowProvider } from "@xyflow/react";
import { useEffect, useMemo, useState } from "react";
import { bindChannels, bindDevices } from "./canvas/binding";
import { Canvas } from "./canvas/Canvas";
import { StationProvider } from "./canvas/context";
import { isPinned, patchNode, pin, unpin } from "./canvas/graph";
import { deviceDialId } from "./canvas/nodes/DeviceFace";
import { Rack } from "./canvas/Rack";
import { StationBar, type View } from "./canvas/StationBar";
import { useHotkeys } from "./canvas/useHotkeys";
import { useStation } from "./canvas/useStation";
import { BTN_PRIMARY } from "./components/controls";
import { TUNE_STEPS_HZ, tuningRange } from "./components/dial";
import { Shortcuts } from "./components/Shortcuts";
import { Toasts } from "./components/Toasts";
import { TokenGate } from "./components/TokenGate";
import {
  BOOKMARKS_KEY,
  CLIENTS_KEY,
  channelTypesQuery,
  clientsQuery,
  DECODER_LOG_KEY,
  DEVICES_KEY,
  PRESETS_KEY,
  patchCatalogQuery,
  RECORDINGS_KEY,
  STATE_KEY,
  stateQuery,
  WORKSPACES_KEY,
} from "./lib/api";
import { audioEngine } from "./lib/audio/useChannelAudio";
import { useDecodedStore } from "./lib/decoded";
import { useScannerStore } from "./lib/scanner";
import { spectrumHub } from "./lib/spectrum";
import { pushToast } from "./lib/toasts";
import type { PatchGraph, ServerEvent, StateScope } from "./lib/types";
import { useChannelPatch } from "./lib/useChannelPatch";
import { useDevicePatch } from "./lib/useDevicePatch";
import { SdrSocket } from "./lib/ws";

/** Modes the `m` shortcut walks, in the order an operator sweeps them. Decoders are not in the
 * ring: swapping a channel to ADS-B mid-listen is a different intent, not the next mode. */
const MODE_RING = ["nfm", "wfm", "am", "ssb"] as const;

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<View>("patch");
  const [stepHz, setStepHz] = useState(100_000);
  const [showShortcuts, setShowShortcuts] = useState(false);

  const state = useQuery(stateQuery());
  const clients = useQuery(clientsQuery());
  const channelTypes = useQuery(channelTypesQuery());
  const catalog = useQuery(patchCatalogQuery());
  const station = useStation();
  const { applyPatch, cachedSettings } = useDevicePatch();
  const { applyEdit } = useChannelPatch();
  // A fresh `[]` every render would defeat every downstream memo, and the binding passes below
  // are the hot ones — they run over the whole patch.
  const deviceSets = useMemo(() => state.data?.device_sets ?? [], [state.data?.device_sets]);

  useEffect(() => {
    const s = new SdrSocket();
    s.onStatus = setConnected;
    // Decoder frames bypass TanStack Query entirely (PLAN §5): under ADS-B traffic they arrive
    // hundreds a second, so they go straight into the batched store. The action identity is
    // stable, so this listener never needs re-registering.
    s.addEventListener(useDecodedStore.getState().observe);
    // Scanner progress is its own high-rate event for the same reason (PLAN §13).
    s.addEventListener(useScannerStore.getState().observe);
    // Spectrum is refcounted per device set so several scope faces share one stream.
    spectrumHub.attach(s);
    audioEngine.attach(s);
    setSocket(s);
    s.connect();
    return () => {
      spectrumHub.detach();
      s.close();
    };
  }, []);

  useEffect(() => {
    if (socket === null) {
      return;
    }
    socket.onEvent = (event: ServerEvent) => {
      switch (event.type) {
        case "Hello":
          void queryClient.invalidateQueries();
          break;
        case "StateChanged":
          invalidateScope(queryClient, event.data.scope);
          break;
        case "Error":
          // The wire carries no coordinates: the audio engine claims errors answering its
          // in-flight subscribes; the rest land in the toast stack.
          if (!audioEngine.claimServerError(event.data.message)) {
            pushToast(event.data.message);
          }
          break;
        default:
          break;
      }
    };
  }, [socket, queryClient]);

  useEffect(() => {
    if (station.error !== null) {
      pushToast(`Station: ${station.error}`);
    }
  }, [station.error]);

  // A patch that names radios which are not attached is normal (CANVAS §3) — but a patch whose
  // channels the engine refused is a station that is not doing what it draws, so it is said out
  // loud rather than left to be noticed.
  useEffect(() => {
    for (const refusal of station.applied?.refused ?? []) {
      pushToast(`${refusal.node}: ${refusal.reason}`);
    }
  }, [station.applied]);

  const snapshot = station.active?.snapshot ?? null;
  const graph: PatchGraph = useMemo(
    () => snapshot?.graph ?? { nodes: [], edges: [] },
    [snapshot?.graph],
  );
  const devices = useMemo(() => bindDevices(graph, deviceSets), [graph, deviceSets]);
  const channels = useMemo(() => bindChannels(graph, devices), [graph, devices]);

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
  const selectedSet =
    selected === null
      ? null
      : (devices.get(selected) ??
        (() => {
          const owner = (graph.edges ?? []).find(
            (edge) => edge.to.node === selected && edge.to.port === "iq",
          );
          return owner === undefined ? null : (devices.get(owner.from.node) ?? null);
        })());

  const channelNodes = graph.nodes.filter((node) => node.kind === "channel");

  useHotkeys({
    tune: (steps) => {
      if (selectedSet === null) {
        return;
      }
      // Clamped like the dial: a radio at the edge of its range should stop there, not send the
      // driver a frequency it will refuse and toast about once per keypress.
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
    // One dial per receiver node, so the binding reaches the *selected* node's dial; with a
    // channel selected it reaches the receiver that channel is wired to.
    focusDial: () => {
      const owner =
        selected !== null && devices.has(selected)
          ? selected
          : (graph.edges ?? []).find((edge) => edge.to.node === selected && edge.to.port === "iq")
              ?.from.node;
      if (owner !== undefined) {
        document.getElementById(deviceDialId(owner))?.focus();
      }
    },
    cycleMode: (direction) => {
      if (selectedSet === null || selectedChannel === null) {
        return;
      }
      const at = MODE_RING.indexOf(
        selectedChannel.settings.params.type as (typeof MODE_RING)[number],
      );
      // A decoder channel is not on the ring; entering it at the first mode is the only sane
      // answer, and the settings for the new mode start at the server's defaults.
      const next = MODE_RING[(Math.max(0, at) + direction + MODE_RING.length) % MODE_RING.length];
      if (next === undefined || selected === null) {
        return;
      }
      // Both halves, or the node and its channel disagree: the engine keeps the channel's id
      // across a type change, but the *node* names the type (CANVAS §4), so a patch left saying
      // `nfm` would unbind this face and the next apply would add a second channel for it.
      applyEdit(selectedSet.id, selectedChannel.id, { params: { type: next, settings: {} } });
      station.save((current) => ({
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
    toggleAudio: () => {
      if (selectedSet === null || selectedChannel === null || socket === null) {
        return;
      }
      if (audioEngine.isPlaying(selectedSet.id, selectedChannel.id)) {
        audioEngine.stop(selectedSet.id, selectedChannel.id);
      } else {
        audioEngine.attach(socket);
        audioEngine.start(selectedSet.id, selectedChannel.id);
      }
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
      station.save((current) => ({
        ...current,
        rack: isPinned(current.rack ?? {}, selectedNode.id)
          ? unpin(current.rack ?? {}, selectedNode.id)
          : pin(current.rack ?? {}, selectedNode.id),
      }));
    },
    toggleView: () => setView((current) => (current === "patch" ? "rack" : "patch")),
    showShortcuts: () => setShowShortcuts(true),
  });

  return (
    <TokenGate onToken={() => socket?.retryNow()}>
      <div className="flex h-full flex-col bg-bg text-ink">
        {socket !== null && snapshot !== null && (
          <StationProvider
            value={{
              socket,
              connected,
              graph,
              rack: snapshot.rack ?? {},
              context,
              deviceSets,
              devices,
              channels,
              selected,
              select: setSelected,
              edit: station.save,
              apply: station.apply,
            }}
          >
            <StationBar
              view={view}
              onView={setView}
              workspaces={station.workspaces}
              activeWorkspace={station.active?.id ?? null}
              onActivate={station.activate}
              onCreate={station.create}
              onRemove={station.remove}
              connected={connected}
              clients={clients.data?.clients ?? 1}
              onShowShortcuts={() => setShowShortcuts(true)}
            />
            {view === "patch" ? (
              <ReactFlowProvider>
                <Canvas />
              </ReactFlowProvider>
            ) : (
              <Rack />
            )}
          </StationProvider>
        )}

        {/* Deleting the last workspace leaves the station with none, honestly (the server says
            so rather than inventing one); the only thing to offer is a new one. */}
        {station.active === null && !station.pending && (
          <div className="flex min-h-0 flex-1 items-center justify-center">
            <button type="button" className={BTN_PRIMARY} onClick={() => station.create("Station")}>
              Create a station
            </button>
          </div>
        )}

        <Shortcuts open={showShortcuts} onOpenChange={setShowShortcuts} />
        <Toasts />
      </div>
    </TokenGate>
  );
}

// PLAN §5: each `StateChanged` scope maps to exactly the query keys it invalidates.
function invalidateScope(queryClient: QueryClient, scope: StateScope): void {
  switch (scope.scope) {
    case "all":
      void queryClient.invalidateQueries();
      break;
    case "devices":
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      void queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
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
      break;
    case "clients":
      void queryClient.invalidateQueries({ queryKey: CLIENTS_KEY });
      break;
    case "workspaces":
      // Covers the list and every open station: the station queries are keyed under this prefix.
      void queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY });
      break;
    case "decoder_log":
      // Only structural changes (cleared, pruned) land here; individual decodes arrive as
      // `Decoded` and are appended client-side, so this never fires per frame.
      void queryClient.invalidateQueries({ queryKey: DECODER_LOG_KEY });
      break;
  }
}
