import { useArrayStore } from "./arrays";
import { useBearingStore } from "./bearings";
import { useFusionStore } from "./fusion";
import { useProcessorStore } from "./processors";
import { useRefusalStore } from "./refusals";
import { useSurveyStore } from "./survey";

const STORES = [
  useProcessorStore,
  useArrayStore,
  useBearingStore,
  useFusionStore,
  useSurveyStore,
  useRefusalStore,
] as const;

export function forgetNodes(ids: readonly string[]): void {
  if (ids.length === 0) {
    return;
  }
  for (const store of STORES) {
    store.getState().forget(ids);
  }
}

export function resetNodeState(): void {
  for (const store of STORES) {
    store.getState().reset();
  }
}
