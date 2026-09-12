import { formatLevel, gateDb, gateOpen, LEVEL_FLOOR_DB, levelUnit } from "../lib/levels";
import type { ChannelLevel } from "../lib/types";
import { SQUELCH_RANGE_DB } from "./channelSettings";

const SEGMENT_DB = 10;
const SEGMENTS = (SQUELCH_RANGE_DB.max - SQUELCH_RANGE_DB.min) / SEGMENT_DB;
const SEGMENT_GAPS = `repeating-linear-gradient(to right, transparent 0, transparent calc(${100 / SEGMENTS}% - 1px), var(--color-panel) calc(${100 / SEGMENTS}% - 1px), var(--color-panel) ${100 / SEGMENTS}%)`;

export function LevelMeter({
  level,
  squelchDb,
}: {
  level: ChannelLevel | undefined;
  squelchDb?: number | null;
}) {
  const floor = SQUELCH_RANGE_DB.min;
  const now = levelUnit(level?.level_db ?? Number.NEGATIVE_INFINITY, floor);
  const peak = levelUnit(level?.peak_db ?? Number.NEGATIVE_INFINITY, floor);
  const gate = gateDb(level, squelchDb);
  const threshold = gate === null ? null : levelUnit(gate, floor);
  const open = gate === null || gateOpen(level, squelchDb);
  const heard = level !== undefined && level.level_db > LEVEL_FLOOR_DB;

  return (
    <div
      className="flex items-center gap-2"
      title={
        gate === null
          ? "Signal in the channel, in dB below full scale"
          : `Signal in the channel; the mark is where the squelch opens${open ? "" : " (closed)"}`
      }
    >
      <div
        className="relative h-1.5 min-w-0 flex-1 rounded-xs bg-panel-2"
        role="meter"
        aria-label="Signal level"
        aria-valuenow={Math.round(now * 100)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuetext={`${formatLevel(level?.level_db)}${gate === null ? "" : open ? ", squelch open" : ", squelch closed"}`}
      >
        <div
          className={`absolute inset-y-0 left-0 rounded-xs ${open ? "bg-accent" : "bg-accent-dim"}`}
          style={{ width: `${now * 100}%` }}
        />
        <div aria-hidden className="absolute inset-0" style={{ backgroundImage: SEGMENT_GAPS }} />
        {peak > 0 && (
          <div
            aria-hidden
            className="absolute inset-y-0 w-px bg-ink-dim"
            style={{ left: `calc(${peak * 100}% - 1px)` }}
          />
        )}
        {threshold !== null && (
          <div
            aria-hidden
            className="absolute -inset-y-0.5 w-0.5 bg-ink"
            style={{ left: `calc(${threshold * 100}% - 1px)` }}
          />
        )}
      </div>
      <span
        className={`w-16 shrink-0 text-right font-mono text-xs tabular-nums ${
          heard ? "text-ink" : "text-ink-faint"
        }`}
      >
        {formatLevel(level?.level_db)}
      </span>
    </div>
  );
}
