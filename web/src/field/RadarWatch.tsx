import { RangeDopplerView } from "../canvas/nodes/RangeDopplerFace";
import { useDfStore } from "../lib/df";
import type { MissionProps } from "./missions";
import { trackLabel } from "./radarTrack";

export function RadarWatch({ node }: MissionProps) {
  const detections = useDfStore((store) => store.byNode[node]?.detections) ?? [];
  const tracked = detections.filter((hit) => hit.track_id != null);
  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1">
        <RangeDopplerView node={node} />
      </div>
      <div className="flex flex-col gap-1 border-line border-t px-3 py-2">
        <span className="text-ink-dim text-xs">
          {tracked.length === 0
            ? `${detections.length} echoes, nothing followed yet`
            : `${tracked.length} of ${detections.length} echoes followed`}
        </span>
        {tracked.slice(0, 6).map((hit) => (
          <span key={hit.track_id} className="font-mono text-sm tabular-nums">
            {trackLabel(hit)}
          </span>
        ))}
      </div>
    </div>
  );
}
