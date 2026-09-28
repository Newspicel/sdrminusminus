import { Button } from "../../components/BaseControls";
import { BTN } from "../../components/controls";
import { Readout, ReadoutRow } from "../../components/Readout";
import { useFusionClear, useFusionSeed, useFusionStore } from "../../lib/fusion";
import type { PatchNode } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { useWorkspaceContext } from "../context";
import { FaceBody, NodeShell } from "./NodeShell";
import { NAV_TEXT, spreadLabel, stationAge } from "./triangulation";

const AGE_TICK_MS = 1_000;

export function TriangulationFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const fusion = useFusionStore((store) => store.byNode[node.id]);
  useFusionSeed(node.id);
  const { clear, pending } = useFusionClear(node.id);
  const now = useNow(AGE_TICK_MS);
  if (node.kind !== "triangulation") {
    return null;
  }
  const estimate = fusion?.estimate ?? null;
  const stations = fusion?.stations ?? [];
  const finders = (workspace.graph.edges ?? []).filter(
    (edge) => edge.to.node === node.id && edge.to.port === "events",
  ).length;
  return (
    <NodeShell
      node={node}
      title="Triangulation"
      category="tool"
      subtitle={`${stations.length} of ${finders} reporting`}
    >
      <FaceBody>
        <div
          className="flex flex-col gap-2 p-2"
          title={finders === 0 ? "Wire in two or more direction finders" : undefined}
        >
          <Readout>
            <ReadoutRow label="Estimate">
              {estimate === null ? "-" : `${estimate.lat.toFixed(5)}, ${estimate.lon.toFixed(5)}`}
            </ReadoutRow>
            <ReadoutRow
              label="Spread"
              title="The long and short axes of the error ellipse the crossing bearings leave"
            >
              {spreadLabel(estimate)}
            </ReadoutRow>
            <ReadoutRow label="Guidance">
              {fusion?.nav === undefined || fusion.nav === null
                ? "-"
                : `${NAV_TEXT[fusion.nav.kind]} · ${Math.round(fusion.nav.bearing_deg)}°`}
            </ReadoutRow>
            <ReadoutRow label="Bearings">{fusion?.samples ?? 0}</ReadoutRow>
          </Readout>
          <div className="flex flex-col gap-1">
            {stations.map((station) => (
              <div
                key={station.station_id}
                className="flex items-baseline justify-between gap-2 text-sm"
              >
                <span className="truncate">{station.station_id}</span>
                <span className="text-ink-dim text-xs">
                  {station.bearings} · {stationAge(station, now)}
                </span>
              </div>
            ))}
          </div>
          <Button
            className={BTN}
            type="button"
            title="Throw away every bearing the grid holds and start crossing again"
            disabled={pending}
            onClick={clear}
          >
            Clear
          </Button>
        </div>
      </FaceBody>
    </NodeShell>
  );
}
