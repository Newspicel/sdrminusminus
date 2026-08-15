import { formatLevel, gateDb, gateOpen, levelUnit } from "../lib/levels";
import type { ChannelLevel } from "../lib/types";

export function LevelMeter({
  level,
  squelchDb,
}: {
  level: ChannelLevel | undefined;
  squelchDb?: number | null;
}) {
  const now = levelUnit(level?.level_db ?? Number.NEGATIVE_INFINITY);
  const peak = levelUnit(level?.peak_db ?? Number.NEGATIVE_INFINITY);
  const gate = gateDb(level, squelchDb);
  const threshold = gate === null ? null : levelUnit(gate);
  const open = gateOpen(level, squelchDb);

  return (
    <div className="flex items-center gap-2">
      <div
        className="relative h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-panel-2"
        role="meter"
        aria-label="Signal level"
        aria-valuenow={Math.round(now * 100)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuetext={formatLevel(level?.level_db)}
      >
        <div
          className={`absolute inset-y-0 left-0 rounded-full ${open ? "bg-accent" : "bg-accent-dim"}`}
          style={{ width: `${now * 100}%` }}
        />
        {peak > 0 && (
          <div
            className="absolute inset-y-0 w-0.5 bg-ink"
            style={{ left: `calc(${peak * 100}% - 1px)` }}
          />
        )}
        {threshold !== null && (
          <div
            className="absolute inset-y-0 w-px bg-line-strong"
            style={{ left: `${threshold * 100}%` }}
          />
        )}
      </div>
      <span className="legend w-16 shrink-0 text-right font-mono tabular-nums">
        {formatLevel(level?.level_db)}
      </span>
    </div>
  );
}
