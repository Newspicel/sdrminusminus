import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { pushToast } from "../lib/toasts";
import type { ChannelParams, DeviceSet } from "../lib/types";
import { useBandPlan } from "../lib/useBandPlan";
import { useDevicePatch } from "../lib/useDevicePatch";
import { searchPlan, serviceEdge, serviceLabel } from "./bandPlan";
import { LABEL } from "./controls";
import { EmptyState } from "./EmptyState";
import { formatHz } from "./format";

/** Enough to fill the drawer without turning a two-letter query into a wall. */
const LIMIT = 30;

export function BandsPanel({ active }: { active: DeviceSet | null }) {
  const { plan } = useBandPlan();
  const { applyPatch } = useDevicePatch();
  const [query, setQuery] = useState("");

  const hits = plan === null ? [] : searchPlan(plan, query, LIMIT);

  /** Tune the receiver to the band's centre. Only the receiver: this panel has no notion of
   * which channel is selected — the scope's ruler is where a band moves a channel — and it is
   * the same thing the bookmarks beside it do. */
  const tune = (startHz: number, stopHz: number, suggested: ChannelParams | null): void => {
    if (active === null) {
      return;
    }
    applyPatch(active.id, { center_hz: startHz + (stopHz - startHz) / 2 });
    if (suggested !== null) {
      pushToast(`${suggested.type.toUpperCase()} is the mode for this band — set it on a channel`);
    }
  };

  return (
    <div className="flex flex-col gap-2 p-3">
      {active === null && (
        <span className="text-sm text-muted-foreground">
          Nothing to tune: select a device node on the canvas first.
        </span>
      )}

      <Input
        placeholder="marine VHF, 70 cm ham, 145.500…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        aria-label="Search the band plan"
      />

      {plan === null && <Skeleton className="h-8 w-full" />}
      {plan !== null && query.trim() === "" && (
        <span className="text-sm text-muted-foreground">
          Search {plan.region.name} by service, band name, wavelength or frequency.
        </span>
      )}
      {plan !== null && query.trim() !== "" && hits.length === 0 && (
        <EmptyState>Nothing in {plan.region.name} matches that.</EmptyState>
      )}

      {hits.map((hit) => {
        const { allocation } = hit;
        return (
          <div key={`${hit.laneId}:${allocation.id}`} className="flex items-start gap-2">
            <Button
              type="button"
              className="min-w-0 flex-1 rounded px-1 py-1 text-left transition-colors hover:bg-muted disabled:opacity-40"
              disabled={active === null}
              onClick={() =>
                tune(allocation.start_hz, allocation.stop_hz, allocation.suggested ?? null)
              }
            >
              <span className="flex items-center gap-1.5">
                <span
                  aria-hidden
                  className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
                />
                <span className="min-w-0 truncate text-sm text-foreground">{allocation.name}</span>
                {allocation.suggested != null && (
                  <Badge variant="secondary">{allocation.suggested.type}</Badge>
                )}
              </span>
              <span className={`${LABEL} mt-0.5`}>
                {formatHz(allocation.start_hz)}–{formatHz(allocation.stop_hz)} ·{" "}
                {serviceLabel(allocation.service)}
                {hit.laneId !== "allocation" && ` · ${hit.laneName}`}
              </span>
              {allocation.notes != null && (
                <p className="mt-0.5 text-xs leading-snug text-muted-foreground">
                  {allocation.notes}
                </p>
              )}
            </Button>
          </div>
        );
      })}
    </div>
  );
}
