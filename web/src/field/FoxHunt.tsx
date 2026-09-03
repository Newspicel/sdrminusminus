import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button } from "../components/BaseControls";
import { formatHz } from "../components/format";
import {
  BEARING_LABEL,
  bearing,
  formatHuntDb,
  huntRefusal,
  huntSettingsOf,
  liveHunt,
} from "../components/hunt";
import { MapPanel } from "../components/MapPanel";
import { STATE_KEY, startHunt, stopHunt } from "../lib/api";
import { type Clicker, startClicker } from "../lib/geiger";
import { useHuntStore } from "../lib/hunt";
import { positionSourcesOf } from "../lib/position";
import { useSignalSurveyStore } from "../lib/signalSurvey";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, PatchGraph } from "../lib/types";
import type { MissionProps } from "./missions";

const INTERVAL_MS = 50;

export function FoxHunt({
  node,
  graph,
  set,
}: MissionProps & { graph: PatchGraph; set: DeviceSet | null }) {
  const queryClient = useQueryClient();
  const pushed = useHuntStore((store) => (set === null ? undefined : store.byDeviceSet[set.id]));
  const clearLive = useHuntStore((store) => store.clear);
  const status = liveHunt(set, pushed);
  const strength = status?.strength ?? 0;
  const settings = status?.settings ?? huntSettingsOf(graph, node);
  const [clicks, setClicks] = useState(true);
  const clicker = useRef<Clicker | null>(null);
  const positionNodes = positionSourcesOf(graph, node);
  const running = status !== null;

  useEffect(() => {
    if (!running || !clicks) {
      clicker.current?.stop();
      clicker.current = null;
      return;
    }
    clicker.current ??= startClicker();
    return () => {
      clicker.current?.stop();
      clicker.current = null;
    };
  }, [running, clicks]);

  useEffect(() => {
    clicker.current?.setStrength(strength);
  }, [strength]);

  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });
  const startMut = useMutation({
    mutationFn: async (deviceSet: number) =>
      startHunt(deviceSet, { ...settings, interval_ms: INTERVAL_MS }),
    onError: (error: Error) => pushToast(error.message),
    onSettled: invalidate,
  });
  const stopMut = useMutation({
    mutationFn: stopHunt,
    onSuccess: (_status, deviceSet) => clearLive(deviceSet),
    onError: (error: Error) => pushToast(error.message),
    onSettled: invalidate,
  });

  const refusal =
    set === null ? "Wire this hunt's control out to a radio." : huntRefusal(set, settings.freq_hz);
  const busy = startMut.isPending || stopMut.isPending;
  const samples = useSignalSurveyStore((store) => store.sessions[node]?.samples ?? []);

  return (
    <div className="flex h-full flex-col">
      <div className="px-3 py-2 text-center">
        <p className="font-mono text-3xl tabular-nums">{formatHz(settings.freq_hz)}</p>
        <p className="text-xs text-ink-dim">
          {status === null
            ? "not hunting"
            : `${formatHuntDb(status.smooth_db)} · ${BEARING_LABEL[bearing(status)]}`}
        </p>
      </div>
      <div className="px-3">
        <div
          className="h-16 w-full overflow-hidden rounded border border-line bg-bg"
          role="meter"
          aria-label="Signal strength"
          aria-valuenow={Math.round(strength * 100)}
        >
          <div
            className="h-full bg-accent transition-[width] duration-100"
            style={{ width: `${strength * 100}%` }}
          />
        </div>
      </div>
      {refusal !== null && <p className="px-3 pt-2 text-center text-danger text-xs">{refusal}</p>}
      <div className="flex justify-center gap-2 px-3 py-2">
        <Button
          type="button"
          disabled={set === null || busy || (!running && refusal !== null)}
          onClick={() => {
            if (set === null) {
              return;
            }
            if (running) {
              stopMut.mutate(set.id);
            } else {
              startMut.mutate(set.id);
            }
          }}
          className={`rounded px-4 py-3 text-sm ${running ? "border border-line" : "bg-accent text-bg"}`}
        >
          {running ? "Stop hunt" : "Start hunt"}
        </Button>
        <Button
          type="button"
          onClick={() => setClicks((on) => !on)}
          className={`rounded px-4 py-3 text-sm ${clicks ? "bg-accent text-bg" : "border border-line"}`}
        >
          {clicks ? "Clicks on" : "Clicks off"}
        </Button>
      </div>
      <div className="min-h-0 flex-1">
        <MapPanel
          kinds={[]}
          positionNodes={positionNodes}
          signalSamples={samples}
          className="h-full w-full"
        />
      </div>
    </div>
  );
}
