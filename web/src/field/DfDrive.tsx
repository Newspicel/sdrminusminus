import { useEffect, useMemo, useRef, useState } from "react";
import { COMPASS_MARKS, polarPoint } from "../canvas/nodes/df";
import { Button } from "../components/BaseControls";
import { MapPanel } from "../components/MapPanel";
import { getRoute } from "../lib/api";
import { useDfStore } from "../lib/df";
import { crossingsFedBy, dfOverlay } from "../lib/dfOverlay";
import { positionSourcesOf, usePositionStore } from "../lib/position";
import type { PatchGraph, Route, RoutePoint } from "../lib/types";
import type { MissionProps } from "./missions";
import {
  formatDistance,
  handoffUrl,
  nextManeuver,
  type RouteState,
  relativeHeading,
  reroutePrompt,
  shouldAnnounce,
} from "./nav";
import { useVoice } from "./useFieldScreen";

const SIZE = 200;
const CENTRE = SIZE / 2;
const RING = SIZE / 2 - 18;

export type NavMode = "auto" | "direct" | "off";

export function DfDrive({
  node,
  graph,
  routing,
}: MissionProps & { graph: PatchGraph; routing: boolean }) {
  const state = useDfStore((store) => store.byNode[node]);
  const positionNodes = positionSourcesOf(graph, node);
  const fix = usePositionStore((store) => store.sources[positionNodes[0] ?? ""]?.fix ?? null);
  const [mode, setMode] = useState<NavMode>("auto");
  const voice = useVoice();
  const here: RoutePoint | null = useMemo(
    () => (fix === null ? null : { lat: fix.latitude, lon: fix.longitude }),
    [fix],
  );
  const crossings = useMemo(() => crossingsFedBy(graph, node), [graph, node]);
  const crossed = useDfStore((store) => store.byNode[crossings[0] ?? ""]);
  const guidance = crossed?.fusion?.guidance ?? null;
  const target = useMemo(() => {
    if (guidance === null) {
      return null;
    }
    if (mode === "direct") {
      const estimate = crossed?.fusion?.estimate;
      return estimate === undefined || estimate === null
        ? { lat: guidance.nav_target.lat, lon: guidance.nav_target.lon }
        : { lat: estimate.lat, lon: estimate.lon };
    }
    return { lat: guidance.nav_target.lat, lon: guidance.nav_target.lon };
  }, [guidance, mode, crossed?.fusion?.estimate]);

  const [route, setRoute] = useState<Route | null>(null);
  const routeState = useRef<RouteState>({ route: null, target: null, mode: null });
  const announced = useRef<string | null>(null);
  const [routeError, setRouteError] = useState<string | null>(null);

  useEffect(() => {
    if (mode === "off" || !routing || here === null || guidance === null || target === null) {
      return;
    }
    const prompt = reroutePrompt(routeState.current, guidance, here);
    if (prompt === "none") {
      return;
    }
    let dropped = false;
    getRoute({ from: here, to: target })
      .then((next) => {
        if (dropped) {
          return;
        }
        routeState.current = { route: next, target, mode: guidance.mode };
        announced.current = null;
        setRoute(next);
        setRouteError(null);
      })
      .catch((error: unknown) => {
        if (!dropped) {
          setRouteError(error instanceof Error ? error.message : "no route");
        }
      });
    return () => {
      dropped = true;
    };
  }, [here, guidance, target, mode, routing]);

  const next = nextManeuver(mode === "off" ? null : route, here);
  useEffect(() => {
    if (shouldAnnounce(next, announced.current) && next !== null) {
      announced.current = next.instruction;
      voice.say(next.instruction);
    }
  }, [next, voice]);

  const overlay = dfOverlay(
    { finders: [node], crossings },
    useDfStore((store) => store.byNode),
    Date.now(),
    here,
  );
  const bearing = state?.reading ?? null;
  const usable = state !== undefined && !state.cal.phase_unknown && (bearing?.confidence ?? 0) > 0;
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: the first touch anywhere arms speech
    <div className="flex h-full flex-col" onPointerDown={voice.arm}>
      <div className="flex items-center justify-between gap-2 px-3 py-2">
        <span className="font-mono text-3xl tabular-nums">
          {usable && bearing !== null ? `${bearing.bearing_deg.toFixed(0).padStart(3, "0")}°` : "—"}
        </span>
        <span className="text-xs text-ink-dim">
          {state === undefined
            ? "waiting"
            : state.cal.phase_unknown
              ? "phase unknown"
              : `${Math.round((bearing?.confidence ?? 0) * 100)}%`}
        </span>
      </div>
      <div className="flex justify-center">
        <Rose
          bearingDeg={usable && bearing !== null ? bearing.bearing_deg : null}
          headingDeg={guidance?.heading_deg ?? null}
          trackDeg={fix?.track_deg ?? null}
        />
      </div>
      <div className="px-3 pb-2 text-center">
        <p className="text-sm">
          {crossings.length === 0
            ? "No guidance: wire this finder's events into a Triangulation node."
            : guidance === null
              ? "Drive until a bearing comes in."
              : guidance.mode === "cross"
                ? `Cross the bearing — steer ${Math.round(guidance.heading_deg)}°`
                : `Close in — ${formatDistance(guidance.distance_m)} to run`}
        </p>
        {next !== null && (
          <p className="mt-1 font-medium text-base">
            {next.instruction} · {formatDistance(next.distanceM)}
          </p>
        )}
        {routeError !== null && mode !== "off" && (
          <p className="mt-1 text-xs text-ink-dim">
            No route ({routeError}) — steering by compass.
          </p>
        )}
      </div>
      <div className="flex items-center justify-between gap-2 px-3 pb-2">
        <div className="flex gap-1">
          {(["auto", "direct", "off"] as const).map((option) => (
            <Button
              key={option}
              type="button"
              onClick={() => setMode(option)}
              className={`rounded px-3 py-2 text-xs ${mode === option ? "bg-accent text-bg" : "border border-line"}`}
            >
              {option}
            </Button>
          ))}
        </div>
        {target !== null && (
          <a
            className="rounded border border-line px-3 py-2 text-xs"
            href={handoffUrl(target, navigator.userAgent)}
          >
            Navigate in Maps
          </a>
        )}
      </div>
      <div className="min-h-0 flex-1">
        <MapPanel kinds={[]} positionNodes={positionNodes} df={overlay} className="h-full w-full" />
      </div>
    </div>
  );
}

function Rose({
  bearingDeg,
  headingDeg,
  trackDeg,
}: {
  bearingDeg: number | null;
  headingDeg: number | null;
  trackDeg: number | null;
}) {
  const needle =
    bearingDeg === null ? null : polarPoint(relativeHeading(bearingDeg, trackDeg), RING, CENTRE);
  const arrow =
    headingDeg === null
      ? null
      : polarPoint(relativeHeading(headingDeg, trackDeg), RING - 24, CENTRE);
  return (
    <svg
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      className="h-[min(48vh,20rem)] w-[min(48vh,20rem)]"
      role="img"
      aria-label="Bearing relative to the vehicle"
    >
      <title>Bearing relative to the vehicle</title>
      <circle cx={CENTRE} cy={CENTRE} r={RING} className="fill-none stroke-line" />
      {COMPASS_MARKS.map((mark) => {
        const at = polarPoint(mark.bearing, RING + 8, CENTRE);
        return (
          <text
            key={mark.label}
            x={at.x}
            y={at.y}
            className="fill-ink-dim text-[8px]"
            textAnchor="middle"
            dominantBaseline="middle"
          >
            {mark.label}
          </text>
        );
      })}
      {needle !== null && (
        <line
          x1={CENTRE}
          y1={CENTRE}
          x2={needle.x}
          y2={needle.y}
          className="stroke-accent stroke-[3]"
        />
      )}
      {arrow !== null && (
        <line
          x1={CENTRE}
          y1={CENTRE}
          x2={arrow.x}
          y2={arrow.y}
          className="stroke-[6] stroke-warn/70"
          strokeLinecap="round"
        />
      )}
      <circle cx={CENTRE} cy={CENTRE} r={3} className="fill-accent" />
    </svg>
  );
}
