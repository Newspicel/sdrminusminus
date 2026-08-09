// App shell (PLAN §10). Owns the WebSocket, turns `StateChanged` events into TanStack Query
// invalidations (the only invalidation path — no polling), and lays out the device bar over the
// spectrum/waterfall with the channel + library panels underneath.
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { BookmarksPanel } from "./components/BookmarksPanel";
import { ChannelsPanel } from "./components/ChannelsPanel";
import { DecoderLogPanel } from "./components/DecoderLogPanel";
import { AprsView, PagerView, RdsView, TargetsView, TextView } from "./components/DecoderPanels";
import { DeviceBar } from "./components/DeviceBar";
import { DeviceSettingsPanel } from "./components/DeviceSettings";
import { FirstRun } from "./components/FirstRun";
import { MapPanel } from "./components/MapPanel";
import { PanelSection } from "./components/PanelSection";
import { PresetsPanel } from "./components/PresetsPanel";
import { RecordingsPanel } from "./components/RecordingsPanel";
import { ScannerPanel } from "./components/ScannerPanel";
import { SpectrumDisplay } from "./components/SpectrumDisplay";
import { TemplatesPanel } from "./components/TemplatesPanel";
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
} from "./lib/api";
import { audioEngine } from "./lib/audio/useChannelAudio";
import { useDecodedStore } from "./lib/decoded";
import { useScannerStore } from "./lib/scanner";
import type { ChannelInfo, ServerEvent, StateScope } from "./lib/types";
import { SdrSocket } from "./lib/ws";

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [activeDs, setActiveDs] = useState<number | null>(null);
  const [selectedChannel, setSelectedChannel] = useState<number | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);

  const state = useQuery(stateQuery());
  const clients = useQuery(clientsQuery());
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
  // Which live views to show follows from what is actually running: a decoder channel on the
  // active set gets its panel, and nothing else takes up space.
  const decoders = (active?.channels ?? []).filter((c) => DECODER_KINDS.has(kindOf(c)));

  return (
    <TokenGate onToken={() => socket?.retryNow()}>
      <div className="flex h-full flex-col bg-bg text-ink">
        <header className="flex items-center justify-between border-b border-line px-4 py-2">
          <div className="flex items-baseline gap-2">
            <span className="font-mono text-lg font-semibold tracking-tight text-accent">
              sdr--
            </span>
            <span className="text-xs text-ink-dim">ops &amp; UX polish · M5</span>
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

        <FirstRun active={active} onSelectDeviceSet={setActiveDs} />

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

        {decoders.length > 0 && (
          <div className="flex shrink-0 flex-col border-t border-line lg:flex-row">
            <div className="min-w-0 flex-1">
              {decoderPanels(active?.id ?? 0, decoders, selectedChannel)}
            </div>
            {/* The map earns its width only when something can be plotted on it. */}
            {decoders.some((c) => MAPPED_KINDS.has(kindOf(c))) && (
              <div className="border-line max-lg:border-t lg:w-[28rem] lg:border-l">
                <PanelSection title="Map">
                  <MapPanel className="h-72" />
                </PanelSection>
              </div>
            )}
          </div>
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
                  <PanelSection title="Scanner" defaultOpen={false}>
                    <ScannerPanel active={active} />
                  </PanelSection>
                  <PanelSection title="Templates" defaultOpen={false}>
                    <TemplatesPanel active={active} />
                  </PanelSection>
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
              {/* Like recordings, the decoder log is a device-independent library. */}
              <PanelSection title="Decoder log" defaultOpen={false}>
                <DecoderLogPanel deviceSets={deviceSets} />
              </PanelSection>
            </div>
          </div>
        )}
      </div>
    </TokenGate>
  );
}

/// Channel type ids that emit decoder events. WFM is here because it carries RDS; the
/// descriptor's `decoder_kind` says so, but the panel choice is per channel type.
const DECODER_KINDS = new Set(["wfm", "pocsag", "adsb", "ais", "aprs", "rtty", "morse"]);
/// The subset whose events carry a position, and therefore justify showing the map.
const MAPPED_KINDS = new Set(["adsb", "ais", "aprs"]);

function kindOf(channel: ChannelInfo): string {
  return channel.settings.params.type;
}

/// One live view per decoder channel, scoped to that channel so two POCSAG receivers on
/// different frequencies do not pour into one list.
function decoderPanels(
  deviceSet: number,
  channels: readonly ChannelInfo[],
  selected: number | null,
) {
  return channels.map((channel) => {
    const kind = kindOf(channel);
    // Channel ids are allocated per device set, so two sets both have a channel 1. Scoping on
    // the id alone would pour one set's frames into the other's panel.
    const scope = { deviceSet, channel: channel.id };
    const title = `${kind.toUpperCase()} · channel ${channel.id}`;
    const open = selected === null || selected === channel.id;
    return (
      <PanelSection key={channel.id} title={title} defaultOpen={open}>
        {kind === "wfm" && <RdsView scope={scope} />}
        {(kind === "adsb" || kind === "ais") && <TargetsView scope={scope} />}
        {kind === "aprs" && <AprsView scope={scope} />}
        {kind === "pocsag" && <PagerView scope={scope} />}
        {(kind === "rtty" || kind === "morse") && <TextView kind={kind} scope={scope} />}
      </PanelSection>
    );
  });
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
    case "decoder_log":
      // Only structural changes (cleared, pruned) land here; individual decodes arrive as
      // `Decoded` and are appended client-side, so this never fires per frame.
      void queryClient.invalidateQueries({ queryKey: DECODER_LOG_KEY });
      break;
  }
}
