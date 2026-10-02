import { AutoToggle } from "../../components/AgcAuto";
import { GainMeter, MeterBar, MeterRow } from "../../components/face/Meter";
import { Unit } from "../../components/Unit";
import { useDebouncedCommit } from "../../components/useDebouncedCommit";
import { ARRAY_LIMITS } from "../../lib/limits";
import type { ArrayStatus } from "../../lib/types";
import { useArrayGain } from "./ArraySettings";
import { type ArrayLaneRow, laneQualityPercent, laneTitle } from "./arrayNode";
import { FaceEmpty } from "./NodeShell";

export const NO_LANES_HINT = "Wire radio lanes in";

const GAIN_DB = ARRAY_LIMITS.gain_db;

export function ArrayLanes({
  node,
  rows,
  status,
}: {
  node: string;
  rows: readonly ArrayLaneRow[];
  status: ArrayStatus | undefined;
}) {
  if (rows.length === 0) {
    return <FaceEmpty hint={NO_LANES_HINT} />;
  }
  return (
    <div className="flex flex-col gap-px border-t border-line p-2">
      <GainRow node={node} status={status} />
      <div role="list" aria-label="Lanes" className="flex flex-col gap-px">
        {rows.map((row) => (
          <LaneRow key={row.port} row={row} />
        ))}
      </div>
    </div>
  );
}

function GainRow({ node, status }: { node: string; status: ArrayStatus | undefined }) {
  const { setGain, pending } = useArrayGain(node);
  const range = status?.gain_range_db;
  const auto = status?.gain.kind === "auto";
  const held = status?.gain.kind === "manual" ? status.gain.db : (status?.gain_db ?? GAIN_DB.min);
  const off = status === undefined || pending;
  const slider = useDebouncedCommit((db) => setGain({ kind: "manual", db }));
  const shown = slider.pending ?? held;
  return (
    <MeterRow
      label="Gain"
      title="One gain for every lane"
      meter={
        <GainMeter
          label="Gain"
          className="min-w-0 flex-1"
          min={Math.max(GAIN_DB.min, range?.min ?? GAIN_DB.min)}
          max={Math.min(GAIN_DB.max, range?.max ?? GAIN_DB.max)}
          step={range?.step ?? 0.1}
          value={shown}
          auto={auto}
          disabled={off || auto}
          onChange={slider.change}
        />
      }
      readout={
        <>
          {shown.toFixed(1)} <Unit symbol="dB" className="text-ink-faint" />
        </>
      }
      trailing={
        <AutoToggle
          label="Auto gain"
          pressed={auto}
          title="Array picks one gain for every lane. Needs a cal source."
          disabled={off}
          onChange={(on) => {
            slider.cancel();
            setGain(on ? { kind: "auto" } : { kind: "manual", db: held });
          }}
        />
      }
    />
  );
}

function LaneRow({ row }: { row: ArrayLaneRow }) {
  const lane = row.status;
  const quality = lane === null ? 0 : laneQualityPercent(lane.coherence);
  return (
    <div role="listitem" title={laneTitle(row)}>
      <MeterRow
        port={row.port}
        label={<span className="truncate font-mono text-[11px] text-port-array">L{row.lane}</span>}
        meter={
          <MeterBar
            label={`Lane ${row.lane} quality`}
            value={quality / 100}
            valueText={lane === null ? undefined : `${quality}%`}
          />
        }
        readout={
          lane === null ? (
            "-"
          ) : (
            <span className={lane.clipping ? "text-danger" : undefined}>
              {lane.gain_db.toFixed(1)} <Unit symbol="dB" className="text-ink-faint" />
            </span>
          )
        }
        trailing={
          <span className="text-right font-mono text-xs tabular-nums text-ink-dim">
            {lane === null ? "-" : `${lane.phase_deg.toFixed(0)}°`}
          </span>
        }
      />
    </div>
  );
}
