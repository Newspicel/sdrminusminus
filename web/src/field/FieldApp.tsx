import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../components/BaseControls";
import { huntDeviceSet } from "../components/hunt";
import { Icon } from "../components/Icon";
import { Toasts } from "../components/Toasts";
import { TokenGate } from "../components/TokenGate";
import { aboutQuery, stateQuery, workspaceQuery, workspacesQuery } from "../lib/api";
import type { PatchGraph } from "../lib/types";
import { useSdrSocket } from "../lib/useSdrSocket";
import { DfDrive } from "./DfDrive";
import { FoxHunt } from "./FoxHunt";
import { fieldPath, type Mission, missionTargets, parseFieldPath } from "./missions";
import { RadarWatch } from "./RadarWatch";
import { useFullscreen, useWakeLock } from "./useFieldScreen";

const MISSIONS: Mission[] = [
  {
    id: "foxhunt",
    title: "Fox hunt",
    blurb: "A radio parked on one frequency: warmer or colder, and a click track to walk to it.",
    nodeKind: "hunt",
    component: () => null,
  },
  {
    id: "df",
    title: "DF drive",
    blurb: "A coherent array: live bearing, where to drive next, and the map underneath.",
    nodeKind: "df",
    component: () => null,
  },
  {
    id: "radar",
    title: "Radar watch",
    blurb: "The range–Doppler surface fullscreen, with whatever the tracker is following listed.",
    nodeKind: "passive_radar",
    component: () => null,
  },
];

export function FieldApp() {
  const queryClient = useQueryClient();
  const [path, setPath] = useState(() => window.location.pathname);
  useEffect(() => {
    const listener = (): void => setPath(window.location.pathname);
    window.addEventListener("popstate", listener);
    return () => window.removeEventListener("popstate", listener);
  }, []);
  const route = parseFieldPath(path);
  useSdrSocket(queryClient, null);
  useWakeLock(route.mission !== null);
  const { full, toggle } = useFullscreen();

  const list = useQuery(workspacesQuery());
  const detail = useQuery(workspaceQuery(list.data?.active ?? null));
  const state = useQuery(stateQuery());
  const about = useQuery(aboutQuery(true));
  const graph: PatchGraph = detail.data?.snapshot.graph ?? { nodes: [], edges: [] };

  const go = (next: string): void => {
    window.history.pushState(null, "", next);
    setPath(next);
  };

  return (
    <TokenGate
      onToken={() => {
        void queryClient.invalidateQueries();
      }}
    >
      <div
        className="flex h-dvh flex-col bg-bg text-ink"
        style={{
          paddingTop: "env(safe-area-inset-top)",
          paddingBottom: "env(safe-area-inset-bottom)",
          paddingLeft: "env(safe-area-inset-left)",
          paddingRight: "env(safe-area-inset-right)",
        }}
      >
        <header className="flex items-center justify-between gap-2 border-line border-b px-3 py-2">
          <Button
            type="button"
            className="text-sm"
            onClick={() => go("/field")}
            disabled={route.mission === null}
          >
            {route.mission === null ? (
              "Field mode"
            ) : (
              <>
                <Icon glyph={ArrowLeft} size={16} />
                Missions
              </>
            )}
          </Button>
          <Button type="button" className="text-xs text-ink-dim" onClick={toggle}>
            {full ? "Exit fullscreen" : "Fullscreen"}
          </Button>
        </header>
        <main className="min-h-0 flex-1">
          {route.mission === null || route.node === null ? (
            <Picker graph={graph} onPick={go} />
          ) : route.mission === "df" ? (
            <DfDrive node={route.node} graph={graph} routing={about.data?.routing ?? false} />
          ) : route.mission === "radar" ? (
            <RadarWatch node={route.node} />
          ) : route.mission === "foxhunt" ? (
            <FoxHunt
              node={route.node}
              graph={graph}
              set={huntDeviceSet(graph, state.data?.device_sets ?? [], route.node)}
            />
          ) : (
            <p className="p-4 text-sm text-ink-dim">No mission by that name.</p>
          )}
        </main>
        <Toasts />
      </div>
    </TokenGate>
  );
}

function Picker({ graph, onPick }: { graph: PatchGraph; onPick: (path: string) => void }) {
  const targets = missionTargets(graph, MISSIONS);
  if (targets.length === 0) {
    return (
      <p className="p-4 text-sm text-ink-dim">
        Nothing in the active workspace can be driven from here yet. Add a signal hunt to walk with,
        or a direction finder to drive to.
      </p>
    );
  }
  return (
    <ul className="flex flex-col gap-2 p-3">
      {targets.map((target) => (
        <li key={`${target.mission.id}:${target.node}`}>
          <Button
            type="button"
            className="w-full rounded border border-line px-4 py-4 text-left"
            onClick={() => onPick(fieldPath(target.mission.id, target.node))}
          >
            <span className="block font-medium text-base">
              {target.mission.title} · {target.label}
            </span>
            <span className="block text-ink-dim text-xs">{target.mission.blurb}</span>
          </Button>
        </li>
      ))}
    </ul>
  );
}
