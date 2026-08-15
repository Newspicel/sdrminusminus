// Band occupancy: which frequencies carry traffic, and when.
//
// A grid rather than a list of numbers, because the question is about the shape of a day. One row
// per frequency bucket, one column per hour; the cell's weight is how much of that hour the
// frequency was in use.
//
// Nothing here tunes anything by itself — clicking a row moves the selected receiver, exactly the
// way the bands and bookmarks panels beside it do.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { occupancyQuery } from "../lib/api";
import type { DeviceSet } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { Button, Input } from "./BaseControls";
import { FIELD, segment } from "./controls";
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

const SORTS: { id: OccupancySort; label: string }[] = [
  { id: "busiest", label: "Busiest" },
  { id: "frequency", label: "Frequency" },
];

/** Sightings a bucket needs before it is reported. Matches the server's own default; stated here
 * because the control that changes it is here. */
const MIN_SAMPLES = 30;

export function OccupancyPanel({ active }: { active: DeviceSet | null }) {
  const { applyPatch } = useDevicePatch();
  const [sort, setSort] = useState<OccupancySort>("busiest");
  const [query, setQuery] = useState("");
  const report = useQuery(occupancyQuery(MIN_SAMPLES));

  const rows = occupancyRows(report.data ?? null, sort, query);

  return (
    <div className="flex flex-col gap-2 p-3">
      {active === null && (
        <span className="text-sm text-ink-dim">
          Nothing to tune: select a device node on the canvas first.
        </span>
      )}

      <div className="flex items-center gap-1">
        {SORTS.map((entry) => (
          <Button
            key={entry.id}
            type="button"
            className={segment(sort === entry.id)}
            aria-pressed={sort === entry.id}
            onClick={() => setSort(entry.id)}
          >
            {entry.label}
          </Button>
        ))}
      </div>

      <Input
        className={FIELD}
        placeholder="145.5, 433…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        aria-label="Filter occupancy by frequency"
      />

      {report.isLoading && <span className="text-sm text-ink-dim">Reading the statistics…</span>}
      {!report.isLoading && !hasOccupancy(report.data ?? null) && (
        <span className="text-sm text-ink-dim">
          Nothing measured yet. Occupancy builds from whatever the receivers are tuned to, so leave
          one running — or start a scan — and come back.
        </span>
      )}

      {rows.length > 0 && (
        <>
          {/* The hour axis, once, above every row. Only every sixth label is printed: twenty-four
              would collide long before the drawer is wide enough for them. */}
          <div className="flex items-center gap-2">
            <span className="w-24 shrink-0" />
            <div className="legend flex min-w-0 flex-1 justify-between text-ink-dim">
              {[0, 6, 12, 18].map((hour) => (
                <span key={hour}>{formatHour(hour)}</span>
              ))}
            </div>
            <span className="w-10 shrink-0" />
          </div>

          {rows.map((bucket) => {
            const peak = busiestHour(bucket);
            return (
              <Button
                key={bucket.freq_hz}
                type="button"
                disabled={active === null}
                className="flex w-full items-center gap-2 rounded-[3px] text-left hover:bg-panel-2 disabled:cursor-default disabled:hover:bg-transparent"
                title={
                  peak === null
                    ? `${formatBucketHz(bucket.freq_hz)}, ${bucket.samples} observations`
                    : `${formatBucketHz(bucket.freq_hz)}, busiest around ${formatHour(peak)}, ${
                        bucket.samples
                      } observations`
                }
                onClick={() => {
                  if (active !== null) {
                    applyPatch(active.id, { center_hz: bucket.freq_hz });
                  }
                }}
              >
                <span className="w-24 shrink-0 font-mono text-xs tabular-nums">
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
                <span className="legend w-10 shrink-0 text-right tabular-nums">
                  {formatDuty(bucket.duty)}
                </span>
              </Button>
            );
          })}

          {(report.data?.buckets.length ?? 0) > rows.length && (
            <span className="legend text-ink-dim">
              {rows.length} of {report.data?.buckets.length} frequencies — the busiest
              {rows.length === MAX_ROWS ? " fit here" : " match"}.
            </span>
          )}
        </>
      )}
    </div>
  );
}
