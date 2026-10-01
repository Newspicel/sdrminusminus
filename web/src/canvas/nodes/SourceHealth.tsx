import type { ReactNode } from "react";
import { FaceStats, Stat } from "../../components/face/Stats";
import { DROPS_HINT, formatCount } from "../../components/format";
import { queueSummary, usePipelineHealth } from "../../lib/pipeline";
import type { DeviceSet } from "../../lib/types";

export function SourceHealth({ set, children }: { set: DeviceSet; children?: ReactNode }) {
  const health = usePipelineHealth((state) => state.health);
  const summary = queueSummary(health, set.id);
  const overruns = set.overruns ?? 0;
  return (
    <FaceStats>
      {summary !== null && (
        <Stat label="Queue" title={summary.detail}>
          {summary.oldestMs.toFixed(0)} ms
        </Stat>
      )}
      {overruns > 0 && (
        <Stat label="Drops" title={DROPS_HINT} tone="warn">
          {formatCount(overruns)}
        </Stat>
      )}
      {children}
    </FaceStats>
  );
}
