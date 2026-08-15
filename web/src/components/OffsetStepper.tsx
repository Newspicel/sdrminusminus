import { Button } from "./BaseControls";
import { clampOffsetHz } from "./channelSettings";
import { BTN } from "./controls";
import { NumberField } from "./NumberField";

/** Mirrored either side of the field, coarsest outermost. */
const DOWN_HZ = [-25_000, -5_000] as const;
const UP_HZ = [5_000, 25_000] as const;

export function OffsetStepper({
  offsetHz,
  limitHz,
  onOffset,
  label = "Offset (kHz)",
}: {
  offsetHz: number;
  /** How far the offset may travel either way, or `null` when nothing bounds it yet. */
  limitHz: number | null;
  onOffset: (hz: number) => void;
  label?: string;
}) {
  const limitKhz = limitHz === null ? undefined : limitHz / 1000;
  const step = (hz: number): void => onOffset(clampOffsetHz(offsetHz + hz, limitHz));
  return (
    <span className="flex items-center gap-1">
      {DOWN_HZ.map((hz) => (
        <StepButton key={hz} hz={hz} onStep={step} />
      ))}
      <NumberField
        label={label}
        value={offsetHz / 1000}
        min={limitKhz === undefined ? undefined : -limitKhz}
        max={limitKhz}
        step={0.5}
        onCommit={(khz) => onOffset(clampOffsetHz(Math.round(khz * 1000), limitHz))}
        className="w-20 text-center"
      />
      {UP_HZ.map((hz) => (
        <StepButton key={hz} hz={hz} onStep={step} />
      ))}
    </span>
  );
}

/** All four the same width, so the field is the middle of the row by measurement and not only by
 * count — `−25k` and `+5k` are different lengths, and sized to their text they put the field
 * visibly off-centre. */
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
