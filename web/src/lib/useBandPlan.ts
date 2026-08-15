import { useQuery } from "@tanstack/react-query";
import { useWorkspaceContext } from "../canvas/context";
import { bandPlanQuery, bandRegionsQuery } from "./api";
import type { BandPlan, BandRegion } from "./types";

export interface BandPlanState {
  region: string | null;
  regions: readonly BandRegion[];
  plan: BandPlan | null;
  ruler: boolean;
  setRegion: (region: string) => void;
  setRuler: (ruler: boolean) => void;
}

export function useBandPlan(): BandPlanState {
  const workspace = useWorkspaceContext();
  const regions = useQuery(bandRegionsQuery());
  const stored = workspace.settings.band_region ?? null;
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
