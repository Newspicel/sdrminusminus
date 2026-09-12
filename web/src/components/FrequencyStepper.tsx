import { Button } from "./BaseControls";
import { type RadioWindow, reachesHz } from "./channelSettings";
import { BTN } from "./controls";
import { formatMhz } from "./format";
import { NumberField } from "./NumberField";
import { TuneTo } from "./TuneTo";

const DOWN_HZ = [-25_000, -5_000] as const;
const UP_HZ = [5_000, 25_000] as const;

export function FrequencyStepper({
  frequencyHz,
  window,
  onTune,
  label = "Frequency (MHz)",
}: {
  frequencyHz: number;
  window: RadioWindow | null;
  onTune: (hz: number) => void;
  label?: string;
}) {
  const step = (hz: number): void => onTune(Math.round(frequencyHz + hz));
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1">
      {DOWN_HZ.map((hz) => (
        <StepButton key={hz} hz={hz} onStep={step} />
      ))}
      <NumberField
        label={label}
        value={frequencyHz / 1e6}
        min={0}
        step={0.005}
        invalid={!reachesHz(frequencyHz, window)}
        onCommit={(mhz) => onTune(Math.round(mhz * 1e6))}
        className="w-28 text-center"
      />
      {UP_HZ.map((hz) => (
        <StepButton key={hz} hz={hz} onStep={step} />
      ))}
      <TuneTo
        title="Type a frequency to listen on"
        hz={frequencyHz}
        hint={
          window === null
            ? "The decoder stays here whatever the radio does"
            : `The radio hears ${formatMhz(window.lowHz)} – ${formatMhz(window.highHz)}`
        }
        resolve={(entered) => (Number.isFinite(entered) && entered > 0 ? entered : null)}
        onTune={onTune}
      />
    </span>
  );
}

function StepButton({ hz, onStep }: { hz: number; onStep: (hz: number) => void }) {
  return (
    <Button
      type="button"
      className={`${BTN} w-11 shrink-0 justify-center px-0 font-mono tabular-nums`}
      onClick={() => onStep(hz)}
    >
      {hz > 0 ? "+" : "−"}
      {Math.abs(hz) / 1000}k
    </Button>
  );
}
