// App shell (PLAN §10, DESIGN.md §5). Owns the WebSocket, turns `StateChanged` events into
// TanStack Query invalidations (the only invalidation path — no polling), and frames the
// workspace in two rows of chrome: the radio on top, the view under it, the dock below.
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { TUNE_STEPS_HZ } from "./components/dial";
import { DIAL_ID } from "./components/FrequencyDial";
import { Shortcuts } from "./components/Shortcuts";
import { Toasts } from "./components/Toasts";
import { TokenGate } from "./components/TokenGate";
import { TopBar, tuningRange } from "./components/TopBar";
import {
  BOOKMARKS_KEY,
  CLIENTS_KEY,
  clientsQuery,
  DECODER_LOG_KEY,
  DEVICES_KEY,
  PRESETS_KEY,
  RECORDINGS_KEY,
  STATE_KEY,
  stateQuery,
  WORKSPACES_KEY,
} from "./lib/api";
import { audioEngine } from "./lib/audio/useChannelAudio";
import { useDecodedStore } from "./lib/decoded";
import { useScannerStore } from "./lib/scanner";
import { pushToast } from "./lib/toasts";
import type { ServerEvent, StateScope } from "./lib/types";
import { useChannelPatch } from "./lib/useChannelPatch";
import { useDevicePatch } from "./lib/useDevicePatch";
import { SdrSocket } from "./lib/ws";
import { ShellProvider } from "./shell/context";
import { TabBar } from "./shell/TabBar";
import { useHotkeys } from "./shell/useHotkeys";
import { useNarrow } from "./shell/useNarrow";
import { useWorkspace } from "./shell/useWorkspace";
import { WorkspaceDock } from "./shell/WorkspaceDock";

/** Modes the `m` shortcut walks, in the order an operator sweeps them. Decoders are not in the
 * ring: swapping a channel to ADS-B mid-listen is a different intent, not the next mode. */
const MODE_RING = ["nfm", "wfm", "am", "ssb"] as const;

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [activeDs, setActiveDs] = useState<number | null>(null);
  const [selectedChannel, setSelectedChannel] = useState<number | null>(null);
  const [stepHz, setStepHz] = useState(100_000);
  const [showShortcuts, setShowShortcuts] = useState(false);

  const state = useQuery(stateQuery());
  const clients = useQuery(clientsQuery());
  const workspace = useWorkspace();
  const narrow = useNarrow();
  const { applyPatch, cachedSettings } = useDevicePatch();
  const { applyEdit } = useChannelPatch();
  const deviceSets = state.data?.device_sets ?? [];

  useEffect(() => {
    const s = new SdrSocket();
    s.onStatus = setConnected;
    // Decoder frames bypass TanStack Query entirely (PLAN §5): under ADS-B traffic they
    // arrive hundreds a second, so they go straight into the batched store. The action
    // identity is stable, so this listener never needs re-registering.
    s.addEventListener(useDecodedStore.getState().observe);
    // Scanner progress is its own high-rate event for the same reason (PLAN §13): a sweep
    // steps several times a second and must not invalidate server state per step.
    s.addEventListener(useScannerStore.getState().observe);
    setSocket(s);
    s.connect();
    return () => s.close();
  }, []);

  useEffect(() => {
    if (!socket) {
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
          // in-flight subscribes (surfaced on the channel row); the rest land in the toast stack.
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
    if (workspace.error !== null) {
      pushToast(`Workspace: ${workspace.error}`);
    }
  }, [workspace.error]);

  // Derive the active device set: the user's selection if it still exists, else the first one.
  // No effect needed — this recomputes whenever the WS-invalidated state query refetches.
  const active = deviceSets.find((d) => d.id === activeDs) ?? deviceSets[0] ?? null;
  // Channel ids are allocated per device set, so a selection made on one set would silently
  // match a different channel on another.
  const [selectionSet, setSelectionSet] = useState<number | null>(null);
  if (selectionSet !== (active?.id ?? null)) {
    setSelectionSet(active?.id ?? null);
    setSelectedChannel(null);
  }

  const snapshot = workspace.active?.snapshot ?? null;
  const tabs = snapshot?.tabs ?? [];
  const tab = tabs.find((t) => t.id === snapshot?.active_tab) ?? tabs[0] ?? null;

  const channels = active?.channels ?? [];
  const selected = channels.find((c) => c.id === selectedChannel) ?? null;

  const tuneCenter = (hz: number): void => {
    if (active === null) {
      return;
    }
    const range = tuningRange(active.capabilities);
    applyPatch(active.id, { center_hz: Math.min(range.max, Math.max(range.min, hz)) });
  };
  const tuneChannel = (ch: number, offsetHz: number): void => {
    if (active !== null) {
      applyEdit(active.id, ch, { offset_hz: offsetHz });
    }
  };

  useHotkeys({
    tune: (steps) => {
      if (active !== null) {
        tuneCenter((cachedSettings(active.id)?.center_hz ?? 0) + steps * stepHz);
      }
    },
    stepBy: (direction) => {
      const at = TUNE_STEPS_HZ.indexOf(stepHz as (typeof TUNE_STEPS_HZ)[number]);
      const next = Math.min(TUNE_STEPS_HZ.length - 1, Math.max(0, at + direction));
      setStepHz(TUNE_STEPS_HZ[next] ?? stepHz);
    },
    focusDial: () => document.getElementById(DIAL_ID)?.focus(),
    cycleMode: (direction) => {
      if (active === null || selected === null) {
        return;
      }
      const at = MODE_RING.indexOf(selected.settings.params.type as (typeof MODE_RING)[number]);
      // A decoder channel is not on the ring; entering it at the first mode is the only sane
      // answer, and the settings for the new mode start at the server's defaults.
      const next = MODE_RING[(Math.max(0, at) + direction + MODE_RING.length) % MODE_RING.length];
      if (next !== undefined) {
        applyEdit(active.id, selected.id, { params: { type: next, settings: {} } });
      }
    },
    adjustSquelch: (deltaDb) => {
      if (active === null || selected === null) {
        return;
      }
      applyEdit(active.id, selected.id, (current) => ({
        squelch_db: Math.min(0, Math.max(-120, (current.squelch_db ?? -60) + deltaDb)),
      }));
    },
    toggleSquelch: () => {
      if (active === null || selected === null) {
        return;
      }
      applyEdit(active.id, selected.id, (current) => ({
        squelch_db: current.squelch_db === null || current.squelch_db === undefined ? -60 : null,
      }));
    },
    toggleAudio: () => {
      if (active === null || selected === null || socket === null) {
        return;
      }
      if (audioEngine.isPlaying(active.id, selected.id)) {
        audioEngine.stop(active.id, selected.id);
      } else {
        audioEngine.attach(socket);
        audioEngine.start(active.id, selected.id);
      }
    },
    selectChannel: (direction) => {
      if (channels.length === 0) {
        return;
      }
      const at = channels.findIndex((c) => c.id === selectedChannel);
      const next = channels[(at + direction + channels.length) % channels.length];
      setSelectedChannel(next?.id ?? null);
    },
    selectTab: (index) => {
      const target = tabs[index];
      if (target !== undefined) {
        workspace.saveSnapshot((current) => ({ ...current, active_tab: target.id }));
      }
    },
    showShortcuts: () => setShowShortcuts(true),
  });

  return (
    <TokenGate onToken={() => socket?.retryNow()}>
      <div className="flex h-full flex-col bg-bg text-ink">
        <TopBar
          active={active}
          deviceSets={deviceSets}
          onSelect={setActiveDs}
          connected={connected}
          clients={clients.data?.clients ?? 1}
          stepHz={stepHz}
          onStepHz={setStepHz}
        />

        <TabBar
          workspaces={workspace.workspaces}
          activeId={workspace.active?.id ?? null}
          snapshot={snapshot}
          onSnapshot={workspace.saveSnapshot}
          onActivate={workspace.activate}
          onCreate={workspace.create}
          onRemove={workspace.remove}
          onShowShortcuts={() => setShowShortcuts(true)}
        />

        {socket && tab !== null && snapshot !== null && (
          <ShellProvider
            value={{
              socket,
              connected,
              deviceSets,
              active,
              setActiveDs,
              selectedChannel,
              setSelectedChannel,
              tuneCenter,
              tuneChannel,
            }}
          >
            <WorkspaceDock
              // A tab switch must rebuild the dock, not diff into it: panel ids repeat across
              // tabs, and dockview would move the live panel instead of creating a second one.
              key={`${workspace.active?.id ?? 0}:${tab.id}`}
              tab={tab}
              readOnly={narrow}
              onChange={(next) =>
                workspace.saveSnapshot((current) => ({
                  ...current,
                  tabs: current.tabs.map((existing) => (existing.id === next.id ? next : existing)),
                }))
              }
            />
          </ShellProvider>
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
      // Covers the list and every open layout: the layout queries are keyed under this prefix.
      void queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY });
      break;
    case "decoder_log":
      // Only structural changes (cleared, pruned) land here; individual decodes arrive as
      // `Decoded` and are appended client-side, so this never fires per frame.
      void queryClient.invalidateQueries({ queryKey: DECODER_LOG_KEY });
      break;
  }
}
