import { useEffect, useRef, useState } from "react";
import { Button } from "../components/BaseControls";
import { formatHz } from "../components/format";
import { MapPanel } from "../components/MapPanel";
import { type Clicker, startClicker } from "../lib/geiger";
import { useLevelStore } from "../lib/levels";
import { positionSourcesOf } from "../lib/position";
import { useSignalSurveyStore } from "../lib/signalSurvey";
import type { ChannelLevel, PatchGraph } from "../lib/types";
import type { MissionProps } from "./missions";

const FLOOR_DB = -110;
const CEILING_DB = -20;

/// Where a level sits between "nothing" and "on top of it", which is what both the meter fill and
/// the click rate are driven from.
export function strengthOf(level: ChannelLevel | undefined): number {
  if (level === undefined || !Number.isFinite(level.peak_db)) {
    return 0;
  }
  return Math.min(1, Math.max(0, (level.peak_db - FLOOR_DB) / (CEILING_DB - FLOOR_DB)));
}

export function FoxHunt({
  node,
  graph,
  binding,
}: MissionProps & {
  graph: PatchGraph;
  binding: { deviceSet: number; channel: number; freqHz: number } | null;
}) {
  const level = useLevelStore((store) =>
    binding === null ? undefined : store.byDeviceSet[binding.deviceSet]?.[binding.channel],
  );
  const strength = strengthOf(level);
  const [clicks, setClicks] = useState(true);
  const clicker = useRef<Clicker | null>(null);
  const positionNodes = positionSourcesOf(graph, node);

  useEffect(() => {
    if (!clicks) {
      clicker.current?.stop();
      clicker.current = null;
      return;
    }
    clicker.current ??= startClicker();
    return () => {
      clicker.current?.stop();
      clicker.current = null;
    };
  }, [clicks]);

  useEffect(() => {
    clicker.current?.setStrength(strength);
  }, [strength]);

  const samples = useSignalSurveyStore((store) => store.sessions[node]?.samples ?? []);
  return (
    <div className="flex h-full flex-col">
      <div className="px-3 py-2 text-center">
        <p className="font-mono text-3xl tabular-nums">
          {binding === null ? "—" : formatHz(binding.freqHz)}
        </p>
        <p className="text-xs text-ink-dim">
          {level === undefined ? "waiting for the channel" : `${level.peak_db.toFixed(1)} dBFS`}
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
      <div className="flex justify-center px-3 py-2">
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
