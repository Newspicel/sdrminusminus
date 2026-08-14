import { useQuery } from "@tanstack/react-query";
import { useWorkspaceContext } from "../canvas/context";
import { bandPlanQuery, bandRegionsQuery } from "./api";
import type { BandPlan, BandRegion } from "./types";

export interface BandPlanState {
  /** `null` only before the region list has arrived and the workspace has chosen none. */
  region: string | null;
  regions: readonly BandRegion[];
  plan: BandPlan | null;
  /** Whether scope faces draw the ruler. */
  ruler: boolean;
  setRegion: (region: string) => void;
  setRuler: (ruler: boolean) => void;
}

export function useBandPlan(): BandPlanState {
  const workspace = useWorkspaceContext();
  const regions = useQuery(bandRegionsQuery());
  const stored = workspace.settings.band_region ?? null;
  // The server's default stands in until the workspace names one, and is never written back: a
  // workspace that has not chosen follows the install, including after the install changes.
  const region = stored ?? regions.data?.default_region ?? null;
  const plan = useQuery(bandPlanQuery(region));

  return {
    region,
    regions: regions.data?.regions ?? [],
    plan: plan.data ?? null,
    ruler: workspace.settings.band_ruler ?? true,
    setRegion: (next) => workspace.editSettings({ band_region: next }),
    setRuler: (next) => workspace.editSettings({ band_ruler: next }),
  };
}
