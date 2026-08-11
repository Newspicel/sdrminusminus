// The band plan as the UI consumes it: the region list, the operator's choice, and that
// region's resolved table (FEATURES §5).
//
// One hook because two very different surfaces need exactly the same three things — the ruler on
// every scope face and the explorer in the library drawer — and because adopting the server's
// default region has to happen wherever the plan is first read, not in whichever of them the
// operator happened to open.
import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { bandPlanQuery, bandRegionsQuery } from "./api";
import { defaultBandRegion, useBandRegion } from "./bandRegion";
import type { BandPlan, BandRegion } from "./types";

export interface BandPlanState {
  /** `null` only before the region list has arrived. */
  region: string | null;
  regions: readonly BandRegion[];
  plan: BandPlan | null;
  /** Whether the operator wants the ruler drawn. */
  ruler: boolean;
}

export function useBandPlan(): BandPlanState {
  const { region, ruler } = useBandRegion();
  const regions = useQuery(bandRegionsQuery());
  const plan = useQuery(bandPlanQuery(region));

  const fallback = regions.data?.default_region;
  useEffect(() => {
    if (fallback !== undefined) {
      defaultBandRegion(fallback);
    }
  }, [fallback]);

  return {
    region,
    regions: regions.data?.regions ?? [],
    plan: plan.data ?? null,
    ruler,
  };
}
