import { useState } from "react";
import type { TuneTarget } from "../canvas/libraryTarget";
import { useBandPlan } from "../lib/useBandPlan";
import { useBandTune } from "../lib/useBandTune";
import { Button, Input } from "./BaseControls";
import { searchPlan, serviceEdge, serviceLabel } from "./bandPlan";
import { Checkbox } from "./Checkbox";
import { CHIP, FIELD, LABEL } from "./controls";
import { formatHz } from "./format";
import { Select } from "./Select";

const LIMIT = 30;

export function BandsPanel({ target }: { target: TuneTarget | null }) {
  const { plan, region, regions, ruler, setRegion, setRuler } = useBandPlan();
  const tune = useBandTune(target);
  const [query, setQuery] = useState("");

  const hits = plan === null ? [] : searchPlan(plan, query, LIMIT);
  const tunable = target !== null && !target.locked;

  return (
    <div className="flex flex-col gap-2 p-3">
      <div className="flex items-center gap-2">
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
      </div>

      <Input
        className={FIELD}
        placeholder="marine VHF, 70 cm ham, 145.500…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        aria-label="Search the band plan"
      />

      {plan === null && <span className="text-sm text-ink-dim">Loading the band plan…</span>}
      {plan !== null && query.trim() !== "" && hits.length === 0 && (
        <span className="text-sm text-ink-dim">Nothing in {plan.region.name} matches that.</span>
      )}
      {!tunable && hits.length > 0 && (
        <span className="text-sm text-ink-dim">
          {target === null ? "Select a Device or decoder to tune." : "Tuning is locked here."}
        </span>
      )}

      {hits.map((hit) => {
        const { allocation } = hit;
        return (
          <div key={`${hit.laneId}:${allocation.id}`} className="flex items-start gap-2">
            <Button
              type="button"
              className="min-w-0 flex-1 rounded px-1 py-1 text-left transition-colors hover:bg-panel-2 disabled:opacity-40"
              disabled={!tunable}
              onClick={() => tune(allocation)}
            >
              <span className="flex items-center gap-1.5">
                <span
                  aria-hidden
                  className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
                />
                <span className="min-w-0 truncate text-sm text-ink">{allocation.name}</span>
                {allocation.suggested != null && (
                  <span className={CHIP}>{allocation.suggested.type}</span>
                )}
              </span>
              <span className={`${LABEL} mt-0.5`}>
                {formatHz(allocation.start_hz)}–{formatHz(allocation.stop_hz)} ·{" "}
                {serviceLabel(allocation.service)}
                {hit.laneId !== "allocation" && ` · ${hit.laneName}`}
              </span>
              {allocation.notes != null && (
                <p className="mt-0.5 text-xs leading-snug text-ink-dim">{allocation.notes}</p>
              )}
            </Button>
          </div>
        );
      })}
    </div>
  );
}
