// App shell (PLAN §10). Owns the WebSocket, turns `StateChanged` events into TanStack Query
// invalidations (the only invalidation path — no polling), and lays out the device bar over the
// spectrum/waterfall.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { DeviceBar } from "./components/DeviceBar";
import { DeviceSettingsPanel } from "./components/DeviceSettings";
import { SpectrumDisplay } from "./components/SpectrumDisplay";
import { DEVICES_KEY, STATE_KEY, stateQuery } from "./lib/api";
import type { ServerEvent } from "./lib/types";
import { SdrSocket } from "./lib/ws";

export function App() {
  const queryClient = useQueryClient();
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [activeDs, setActiveDs] = useState<number | null>(null);

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
          void queryClient.invalidateQueries({ queryKey: STATE_KEY });
          if (event.data.scope.scope === "devices") {
            void queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
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
          <span className="text-xs text-ink-dim">real hardware · M1</span>
        </div>
        <div className="flex items-center gap-2 text-xs text-ink-dim">
          <span
            className={`inline-block h-2 w-2 rounded-full ${connected ? "bg-accent" : "bg-danger"}`}
          />
          {connected ? "connected" : "reconnecting…"}
        </div>
      </header>

      <div className="border-b border-line px-4 py-3">
        {socket && <DeviceBar active={active} onSelect={setActiveDs} />}
      </div>

      {active && <DeviceSettingsPanel active={active} />}

      {socket && (
        <SpectrumDisplay socket={socket} deviceSet={active?.id ?? null} connected={connected} />
      )}
    </div>
  );
}
