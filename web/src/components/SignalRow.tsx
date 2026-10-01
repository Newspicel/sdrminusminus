import { useState } from "react";
import { formatLevel, gateDb, gateOpen, levelUnit } from "../lib/levels";
import type { ChannelLevel, Squelch } from "../lib/types";
import { AutoToggle } from "./AgcAuto";
import {
  AUDIO_DEFAULTS,
  DEFAULT_SQUELCH_DB,
  SQUELCH_RANGE_DB,
  squelchAt,
  squelchLevelDb,
  squelchMarginDb,
  squelchMode,
} from "./channelSettings";
import { MeterBar, MeterRow, MeterSlider } from "./face/Meter";
import { useDebouncedCommit } from "./useDebouncedCommit";

const FLOOR = SQUELCH_RANGE_DB.min;

function levelFill(level: ChannelLevel | undefined, open: boolean) {
  return {
    level: levelUnit(level?.level_db ?? Number.NEGATIVE_INFINITY, FLOOR),
    peak: levelUnit(level?.peak_db ?? Number.NEGATIVE_INFINITY, FLOOR),
    className: open ? "bg-accent" : "bg-accent-dim",
  };
}

function LevelBar({ level }: { level: ChannelLevel | undefined }) {
  const fill = levelFill(level, true);
  return (
    <MeterBar
      label="Signal level"
      value={fill.level}
      peak={fill.peak}
      valueText={formatLevel(level?.level_db)}
    />
  );
}

export function SignalRow({
  level,
  squelch,
  onSquelch,
}: {
  level: ChannelLevel | undefined;
  squelch?: Squelch;
  onSquelch?: (squelch: Squelch) => void;
}) {
  const readout = formatLevel(level?.level_db);
  if (onSquelch === undefined) {
    return (
      <MeterRow
        label="Signal"
        title="Signal in the channel, in dB below full scale"
        meter={<LevelBar level={level} />}
        readout={readout}
      />
    );
  }
  return <SquelchRow level={level} squelch={squelch} readout={readout} onSquelch={onSquelch} />;
}

function SquelchRow({
  level,
  squelch,
  readout,
  onSquelch,
}: {
  level: ChannelLevel | undefined;
  squelch: Squelch | undefined;
  readout: string;
  onSquelch: (squelch: Squelch) => void;
}) {
  const mode = squelchMode(squelch);
  const [heldDb, setHeldDb] = useState(DEFAULT_SQUELCH_DB);
  const marginDb = squelchMarginDb(squelch) ?? AUDIO_DEFAULTS.squelchAutoMarginDb;
  const gate = gateDb(level, squelchLevelDb(squelch));
  const held = () => ({ levelDb: gate ?? heldDb, marginDb });
  const slider = useDebouncedCommit((db) =>
    onSquelch(db <= FLOOR ? squelchAt("off", held()) : { mode: "manual", level_db: db }),
  );
  const shown = slider.pending ?? (mode === "off" ? FLOOR : (gate ?? heldDb));
  const open = mode === "off" || gateOpen(level, squelchLevelDb(squelch));
  const auto = mode === "auto" && slider.pending === null;
  return (
    <MeterRow
      label="Squelch"
      meter={
        <MeterSlider
          label="Squelch threshold"
          className="min-w-0 flex-1"
          min={FLOOR}
          max={SQUELCH_RANGE_DB.max}
          step={1}
          value={shown}
          auto={auto}
          fill={levelFill(level, open)}
          title={`Fill: signal. Handle: where the squelch opens${mode === "off" ? ". Far left: off" : ""}`}
          onChange={(db) => {
            if (db > FLOOR) {
              setHeldDb(db);
            }
            slider.change(db);
          }}
        />
      }
      readout={<span className={open ? "" : "text-ink-faint"}>{readout}</span>}
      trailing={
        <AutoToggle
          label="Automatic squelch"
          pressed={mode === "auto"}
          title={
            mode === "auto"
              ? `Opens ${marginDb} dB above the noise floor`
              : "Open a margin above the measured noise floor"
          }
          onChange={(on) => {
            slider.cancel();
            onSquelch(squelchAt(on ? "auto" : "manual", held()));
          }}
        />
      }
    />
  );
}
