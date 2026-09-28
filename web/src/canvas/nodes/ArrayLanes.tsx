import { TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { type ArrayLaneRow, laneQualityPercent, laneTitle } from "./arrayNode";
import { FaceEmpty } from "./NodeShell";

export const NO_LANES_HINT = "Wire radio lanes in";

const HEADERS = ["Lane", "Source", "Phase", "Gain", "Delay", "Q"] as const;

export function ArrayLanes({ rows }: { rows: readonly ArrayLaneRow[] }) {
  if (rows.length === 0) {
    return <FaceEmpty hint={NO_LANES_HINT} />;
  }
  return (
    <table aria-label="Lanes" className="w-full table-fixed border-t border-line">
      <colgroup>
        <col className="w-10" />
        <col />
        <col className="w-15" />
        <col className="w-17" />
        <col className="w-13" />
        <col className="w-12" />
      </colgroup>
      <thead>
        <tr>
          {HEADERS.map((header) => (
            <th key={header} scope="col" className={TABLE_HEAD}>
              {header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <LaneRow key={row.port} row={row} />
        ))}
      </tbody>
    </table>
  );
}

function LaneRow({ row }: { row: ArrayLaneRow }) {
  const lane = row.status;
  const quality = lane === null ? null : laneQualityPercent(lane.coherence);
  return (
    <tr title={laneTitle(row)} className="border-t border-line/60">
      <td className={TABLE_CELL}>{row.lane}</td>
      <td className={`${TABLE_CELL} truncate text-ink-dim`}>{row.sourceLabel}</td>
      <td className={TABLE_CELL}>{lane === null ? "-" : `${lane.phase_deg.toFixed(1)}°`}</td>
      <td className={`${TABLE_CELL} ${lane?.clipping === true ? "text-danger" : ""}`}>
        {lane === null ? "-" : `${lane.gain_db.toFixed(1)} dB`}
      </td>
      <td className={TABLE_CELL}>{lane === null ? "-" : lane.delay_samples.toFixed(2)}</td>
      <td className={TABLE_CELL}>
        {quality === null ? (
          "-"
        ) : (
          <span
            role="meter"
            aria-label={`Lane ${row.lane} quality`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={quality}
            className="block h-1.5 w-full rounded-full bg-line"
          >
            <span
              className="block h-full rounded-full bg-accent"
              style={{ width: `${quality}%` }}
            />
          </span>
        )}
      </td>
    </tr>
  );
}
