import { Button } from "../../components/BaseControls";
import { BTN } from "../../components/controls";
import { Readout, ReadoutRow } from "../../components/Readout";
import { resetFusion } from "../../lib/api";
import { useDfStore } from "../../lib/df";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { GUIDANCE_TEXT, spreadLabel, stationAge } from "./triangulation";

export function TriangulationFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useDfStore((store) => store.byNode[node.id]);
  if (node.kind !== "triangulation") {
    return null;
  }
  const fusion = state?.fusion;
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
      live={estimate !== null}
    >
      <FaceBody>
        {finders === 0 ? (
          <FaceEmpty>Wire the events of two or more direction finders in.</FaceEmpty>
        ) : (
          <div className="flex flex-col gap-2 p-2">
            <Readout>
              <ReadoutRow label="Estimate">
                {estimate === null ? "—" : `${estimate.lat.toFixed(5)}, ${estimate.lon.toFixed(5)}`}
              </ReadoutRow>
              <ReadoutRow
                label="Spread"
                title="The long and short axes of the error ellipse the crossing bearings leave"
              >
                {spreadLabel(estimate)}
              </ReadoutRow>
              <ReadoutRow label="Guidance">
                {fusion?.guidance === undefined || fusion.guidance === null
                  ? "—"
                  : `${GUIDANCE_TEXT[fusion.guidance.mode]} · ${Math.round(fusion.guidance.heading_deg)}°`}
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
                    {station.bearings} · {stationAge(station, Date.now())}
                  </span>
                </div>
              ))}
            </div>
            <Button
              className={BTN}
              type="button"
              title="Throw away every bearing the grid holds and start crossing again"
              onClick={() => {
                void resetFusion(node.id);
              }}
            >
              Clear
            </Button>
          </div>
        )}
      </FaceBody>
    </NodeShell>
  );
}
