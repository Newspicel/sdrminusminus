// App shell (PLAN §10). Owns the WebSocket, turns `StateChanged` events into TanStack Query
// invalidations (the only invalidation path — no polling), and lays out the device bar over the
// spectrum/waterfall with the channel + library panels underneath.
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { BookmarksPanel } from "./components/BookmarksPanel";
import { ChannelsPanel } from "./components/ChannelsPanel";
import { DeviceBar } from "./components/DeviceBar";
import { DeviceSettingsPanel } from "./components/DeviceSettings";
import { PanelSection } from "./components/PanelSection";
import { PresetsPanel } from "./components/PresetsPanel";
import { RecordingsPanel } from "./components/RecordingsPanel";
import { SpectrumDisplay } from "./components/SpectrumDisplay";
import {
  BOOKMARKS_KEY,
  DEVICES_KEY,
  PRESETS_KEY,
  RECORDINGS_KEY,
  STATE_KEY,
  stateQuery,
} from "./lib/api";
import { audioEngine } from "./lib/audio/useChannelAudio";
import type { ServerEvent, StateScope } from "./lib/types";
import { SdrSocket } from "./lib/ws";

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [activeDs, setActiveDs] = useState<number | null>(null);
  const [selectedChannel, setSelectedChannel] = useState<number | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);

  const state = useQuery(stateQuery());
  const deviceSets = state.data?.device_sets ?? [];

  useEffect(() => {
    const s = new SdrSocket();
    s.onStatus = setConnected;
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

  return (
    <div className="flex h-full flex-col bg-bg text-ink">
      <header className="flex items-center justify-between border-b border-line px-4 py-2">
        <div className="flex items-baseline gap-2">
          <span className="font-mono text-lg font-semibold tracking-tight text-accent">sdr--</span>
          <span className="text-xs text-ink-dim">record &amp; replay · M3</span>
        </div>
        <div className="flex items-center gap-2 text-xs text-ink-dim">
          <span
            className={`inline-block h-2 w-2 rounded-full ${connected ? "bg-accent" : "bg-danger"}`}
          />
          {connected ? "connected" : "reconnecting…"}
        </div>
      </header>

      {serverError !== null && (
        <div className="border-b border-line px-4 py-2">
          <div
            role="alert"
            className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
          >
            <span>Server error: {serverError}</span>
            <button
              type="button"
              className="shrink-0 underline"
              onClick={() => setServerError(null)}
            >
              dismiss
            </button>
          </div>
        </div>
      )}

      <div className="border-b border-line px-4 py-3">
        {socket && <DeviceBar active={active} onSelect={setActiveDs} />}
      </div>

      {active && <DeviceSettingsPanel active={active} />}

      {socket && (
        <SpectrumDisplay
          socket={socket}
          deviceSet={active?.id ?? null}
          connected={connected}
          channels={active?.channels ?? []}
          selectedChannel={selectedChannel}
          onSelectChannel={setSelectedChannel}
        />
      )}

      {socket && (
        <div className="flex max-h-[45dvh] shrink-0 flex-col overflow-y-auto border-t border-line md:flex-row md:overflow-hidden">
          {active && (
            <div className="min-w-0 flex-1 md:overflow-y-auto">
              <PanelSection title="Channels">
                <ChannelsPanel
                  socket={socket}
                  deviceSet={active}
                  selected={selectedChannel}
                  onSelect={setSelectedChannel}
                />
              </PanelSection>
            </div>
          )}
          {/* Recordings are a device-independent library: they must stay browsable (and
              playable — Play opens a set) with zero device sets open, unlike the set-bound
              panels above. */}
          <div
            className={`shrink-0 md:overflow-y-auto ${
              active ? "border-line max-md:border-t md:w-80 md:border-l" : "min-w-0 flex-1"
            }`}
          >
            {active && (
              <>
                <PanelSection title="Presets" defaultOpen={false}>
                  <PresetsPanel active={active} />
                </PanelSection>
                <PanelSection title="Bookmarks" defaultOpen={false}>
                  <BookmarksPanel active={active} />
                </PanelSection>
              </>
            )}
            <PanelSection title="Recordings" defaultOpen={false}>
              <RecordingsPanel onSelect={setActiveDs} />
            </PanelSection>
          </div>
        </div>
      )}
    </div>
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
  }
}
