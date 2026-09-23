import { useState } from "react";
import type { TuneTarget } from "../canvas/libraryTarget";
import { useBandPlan } from "../lib/useBandPlan";
import { useBandTune } from "../lib/useBandTune";
import { searchPlan, serviceEdge, serviceLabel } from "./bandPlan";
import { Checkbox } from "./Checkbox";
import { CHIP_SM, LABEL } from "./controls";
import { formatHz } from "./format";
import { List, ListRow, Panel, PanelHint, PanelToolbar, SearchField } from "./ListPanel";
import { Select } from "./Select";

const LIMIT = 30;

export function BandsPanel({ target }: { target: TuneTarget | null }) {
  const { plan, region, regions, ruler, setRegion, setRuler } = useBandPlan();
  const tune = useBandTune(target);
  const [query, setQuery] = useState("");

  const hits = plan === null ? [] : searchPlan(plan, query, LIMIT);
  const tunable = target !== null && !target.locked;

  return (
    <Panel>
      <PanelToolbar>
        <Select
          label="Band plan region"
          value={region ?? ""}
          options={regions.map((entry) => ({ value: entry.id, label: entry.name }))}
          onChange={setRegion}
        />
        <label className={`${LABEL} shrink-0 gap-1.5`}>
          <Checkbox label="Draw the ruler on every scope" checked={ruler} onChange={setRuler} />
          Ruler
        </label>
      </PanelToolbar>
      <SearchField
        placeholder="marine VHF, 70 cm ham, 145.500…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        aria-label="Search the band plan"
      />
      {plan === null && <PanelHint>Loading the band plan…</PanelHint>}
      {plan !== null && query.trim() !== "" && hits.length === 0 && (
        <PanelHint>Nothing in {plan.region.name} matches that.</PanelHint>
      )}
      {!tunable && hits.length > 0 && (
        <PanelHint>
          {target === null ? "Select a device or decoder to tune." : "Tuning is locked here."}
        </PanelHint>
      )}
      {hits.length > 0 && (
        <List>
          {hits.map((hit) => {
            const { allocation } = hit;
            return (
              <ListRow
                key={`${hit.laneId}:${allocation.id}`}
                lead={
                  <span
                    aria-hidden
                    className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
                  />
                }
                primary={allocation.name}
                badge={
                  allocation.suggested == null ? undefined : (
                    <span className={CHIP_SM}>{allocation.suggested.type}</span>
                  )
                }
                secondary={[
                  `${formatHz(allocation.start_hz)}–${formatHz(allocation.stop_hz)}`,
                  serviceLabel(allocation.service),
                  hit.laneId === "allocation" ? "" : hit.laneName,
                ]
                  .filter((part) => part !== "")
                  .join(" · ")}
                hint={allocation.notes ?? undefined}
                disabled={!tunable}
                onSelect={() => tune(allocation)}
              />
            );
          })}
        </List>
      )}
    </Panel>
  );
}
