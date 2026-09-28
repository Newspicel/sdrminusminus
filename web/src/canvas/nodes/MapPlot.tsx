import { useState } from "react";
import { MapPanel } from "../../components/MapPanel";
import { useBearingStore } from "../../lib/bearings";
import { dfOverlay, type OverlaySources } from "../../lib/dfOverlay";
import { recordEvent } from "../../lib/diagnostics";
import type { FusionGridFrame } from "../../lib/frame";
import { useFusionStore } from "../../lib/fusion";
import type { MapKind } from "../../lib/map/layers";
import { usePositionStore } from "../../lib/position";
import { useProcessorStore } from "../../lib/processors";
import { useNow } from "../../lib/useNow";
import { useSurface } from "../../lib/useSurface";
import { useFaceActive } from "./NodeShell";

const AGE_TICK_MS = 1_000;

interface HeldHeat {
  node: string;
  frame: FusionGridFrame;
}

function HeatFeed({ node, onFrame }: { node: string; onFrame: (held: HeldHeat) => void }) {
  useSurface(node, (surface) => {
    if (surface.kind === "fusion_grid") {
      onFrame({ node, frame: surface.frame });
    } else {
      recordEvent("warn", "map", `unexpected ${surface.kind} surface`);
    }
  });
  return null;
}

export function MapPlot({
  kinds,
  positionNodes,
  sources,
}: {
  kinds: readonly MapKind[];
  positionNodes: readonly string[];
  sources: OverlaySources;
}) {
  const bearings = useBearingStore((store) => store.byNode);
  const fusion = useFusionStore((store) => store.byNode);
  const processors = useProcessorStore((store) => store.byNode);
  const here = usePositionStore((store) =>
    positionNodes.length === 0 ? undefined : store.sources[positionNodes[0] ?? ""]?.fix,
  );
  const now = useNow(AGE_TICK_MS);
  const active = useFaceActive();
  const [held, setHeld] = useState<HeldHeat | null>(null);
  const crossing = sources.crossings[0] ?? null;
  const from = here == null ? null : { lat: here.latitude, lon: here.longitude };
  const df = dfOverlay(sources, bearings, fusion, processors, now, from);
  return (
    <>
      {crossing !== null && <HeatFeed key={crossing} node={crossing} onFrame={setHeld} />}
      <MapPanel
        kinds={kinds}
        positionNodes={positionNodes}
        df={df}
        heat={crossing === null ? undefined : held?.node === crossing ? held.frame : null}
        active={active}
        className="h-full min-h-0 w-full flex-1"
      />
    </>
  );
}
