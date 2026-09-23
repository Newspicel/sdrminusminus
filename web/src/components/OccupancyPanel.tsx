import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { tuneDelta } from "../canvas/nodes/deviceNode";
import { occupancyQuery } from "../lib/api";
import type { DeviceSet } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import type { Options } from "./controls";
import { List, ListRow, Panel, PanelHint, PanelToolbar, SearchField } from "./ListPanel";
import {
  busiestHour,
  dutyAlpha,
  formatBucketHz,
  formatDuty,
  formatHour,
  HOURS,
  hasOccupancy,
  MAX_ROWS,
  type OccupancySort,
  occupancyRows,
} from "./occupancy";
import { Segmented } from "./Segmented";

const SORTS: Options<OccupancySort> = [
  { value: "busiest", label: "Busiest" },
  { value: "frequency", label: "Frequency" },
];

const MIN_SAMPLES = 30;

export function OccupancyPanel({ active }: { active: DeviceSet | null }) {
  const { applyPatch } = useDevicePatch();
  const [sort, setSort] = useState<OccupancySort>("busiest");
  const [query, setQuery] = useState("");
  const report = useQuery(occupancyQuery(MIN_SAMPLES));

  const rows = occupancyRows(report.data ?? null, sort, query);

  return (
    <Panel>
      {active === null && <PanelHint>Select a device node first.</PanelHint>}
      <PanelToolbar>
        <Segmented label="Sort occupancy" value={sort} options={SORTS} onChange={setSort} />
        <SearchField
          placeholder="145.5, 433…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          aria-label="Filter occupancy by frequency"
        />
      </PanelToolbar>
      {report.isLoading && <PanelHint>Reading the statistics…</PanelHint>}
      {!report.isLoading && !hasOccupancy(report.data ?? null) && (
        <PanelHint>Nothing measured yet.</PanelHint>
      )}
      {rows.length > 0 && (
        <List
          aside={
            <span className="legend flex flex-1 justify-between pr-12 pl-24">
              {[0, 6, 12, 18].map((hour) => (
                <span key={hour}>{formatHour(hour)}</span>
              ))}
            </span>
          }
        >
          {rows.map((bucket) => {
            const peak = busiestHour(bucket);
            return (
              <ListRow
                key={bucket.freq_hz}
                primary={
                  <span className="flex items-center gap-2">
                    <span className="w-20 shrink-0 tabular-nums">
                      {formatBucketHz(bucket.freq_hz)}
                    </span>
                    <span className="flex min-w-0 flex-1 gap-px">
                      {Array.from({ length: HOURS }, (_, hour) => (
                        <span
                          key={hour}
                          className="h-3 min-w-0 flex-1 rounded-[1px] bg-accent"
                          style={{ opacity: dutyAlpha(bucket.by_hour[hour] ?? 0) }}
                        />
                      ))}
                    </span>
                    <span className="w-10 shrink-0 text-right text-ink-dim tabular-nums">
                      {formatDuty(bucket.duty)}
                    </span>
                  </span>
                }
                hint={
                  peak === null
                    ? `${formatBucketHz(bucket.freq_hz)}, ${bucket.samples} observations`
                    : `${formatBucketHz(bucket.freq_hz)}, busiest around ${formatHour(peak)}, ${bucket.samples} observations`
                }
                disabled={active === null}
                onSelect={() => {
                  if (active !== null) {
                    applyPatch(active.id, tuneDelta(active.capabilities, 0, bucket.freq_hz));
                  }
                }}
              />
            );
          })}
        </List>
      )}
      {rows.length > 0 && (report.data?.buckets.length ?? 0) > rows.length && (
        <PanelHint>
          {rows.length} of {report.data?.buckets.length} frequencies, the busiest
          {rows.length === MAX_ROWS ? " that fit" : " that match"}.
        </PanelHint>
      )}
    </Panel>
  );
}
