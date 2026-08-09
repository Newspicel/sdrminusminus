// App shell (PLAN §10). Owns the WebSocket, turns `StateChanged` events into TanStack Query
// invalidations (the only invalidation path — no polling), and frames the workspace: chrome on
// top (device bar and its settings strip), the server-persisted panel layout underneath.
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { DeviceBar } from "./components/DeviceBar";
import { DeviceSettingsPanel } from "./components/DeviceSettings";
import { FirstRun } from "./components/FirstRun";
import { TokenGate } from "./components/TokenGate";
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
import type { ServerEvent, StateScope } from "./lib/types";
import { SdrSocket } from "./lib/ws";
import { ShellProvider } from "./shell/context";
import { useNarrow } from "./shell/useNarrow";
import { useWorkspace } from "./shell/useWorkspace";
import { WorkspaceBar } from "./shell/WorkspaceBar";
import { WorkspaceDock } from "./shell/WorkspaceDock";

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [activeDs, setActiveDs] = useState<number | null>(null);
  const [selectedChannel, setSelectedChannel] = useState<number | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);

  const state = useQuery(stateQuery());
  const clients = useQuery(clientsQuery());
  const workspace = useWorkspace();
  const narrow = useNarrow();
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
          // in-flight subscribes (surfaced on the channel row); the rest surface here.
          if (!audioEngine.claimServerError(event.data.message)) {
            setServerError(event.data.message);
          }
          break;
        default:
          break;
      }
    };
  }, [socket, queryClient]);

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

  return (
    <TokenGate onToken={() => socket?.retryNow()}>
      <div className="flex h-full flex-col bg-bg text-ink">
        <header className="flex items-center justify-between border-b border-line px-4 py-2">
          <div className="flex items-baseline gap-2">
            <span className="font-mono text-lg font-semibold tracking-tight text-accent">
              sdr--
            </span>
            <span className="text-xs text-ink-dim">the UI shell · M6</span>
          </div>
          <div className="flex items-center gap-2 text-xs text-ink-dim">
            {/* Only worth saying when someone else is here: a solo operator does not need a
                client count, but "another browser is driving this radio" explains a lot. */}
            {(clients.data?.clients ?? 0) > 1 && (
              <span className="font-mono">{clients.data?.clients} clients</span>
            )}
            <span
              className={`inline-block h-2 w-2 rounded-full ${connected ? "bg-accent" : "bg-danger"}`}
            />
            {connected ? "connected" : "reconnecting…"}
          </div>
        </header>

        {(serverError !== null || workspace.error !== null) && (
          <div className="border-b border-line px-4 py-2">
            <div
              role="alert"
              className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
            >
              <span>{serverError ?? `Workspace: ${workspace.error}`}</span>
              {serverError !== null && (
                <button
                  type="button"
                  className="shrink-0 underline"
                  onClick={() => setServerError(null)}
                >
                  dismiss
                </button>
              )}
            </div>
          </div>
        )}

        <FirstRun active={active} onSelectDeviceSet={setActiveDs} />

        <div className="border-b border-line px-4 py-3">
          {socket && <DeviceBar active={active} onSelect={setActiveDs} />}
        </div>

        {active && <DeviceSettingsPanel active={active} />}

        <WorkspaceBar
          workspaces={workspace.workspaces}
          activeId={workspace.active?.id ?? null}
          snapshot={snapshot}
          onSnapshot={workspace.saveSnapshot}
          onActivate={workspace.activate}
          onCreate={workspace.create}
          onRemove={workspace.remove}
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
            }}
          >
            <WorkspaceDock
              // A tab switch must rebuild the dock, not diff into it: panel ids repeat across
              // tabs, and dockview would move the live panel instead of creating a second one.
              key={`${workspace.active?.id ?? 0}:${tab.id}`}
              tab={tab}
              readOnly={narrow}
              onChange={(next) =>
                workspace.saveSnapshot({
                  ...snapshot,
                  tabs: snapshot.tabs.map((existing) =>
                    existing.id === next.id ? next : existing,
                  ),
                })
              }
            />
          </ShellProvider>
        )}
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
