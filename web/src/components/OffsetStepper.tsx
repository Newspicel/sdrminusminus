import { useState } from "react";
import { Button, Form, Input } from "./BaseControls";
import { clampOffsetHz, offsetForFrequencyHz } from "./channelSettings";
import { BTN, BTN_PRIMARY, FIELD, LABEL } from "./controls";
import { formatMhz, parseFrequencyHz } from "./format";
import { NumberField } from "./NumberField";
import { Popover } from "./Popover";

const DOWN_HZ = [-25_000, -5_000] as const;
const UP_HZ = [5_000, 25_000] as const;

export function OffsetStepper({
  offsetHz,
  limitHz,
  centerHz = null,
  onOffset,
  label = "Offset (kHz)",
}: {
  offsetHz: number;
  limitHz: number | null;
  centerHz?: number | null;
  onOffset: (hz: number) => void;
  label?: string;
}) {
  const limitKhz = limitHz === null ? undefined : limitHz / 1000;
  const step = (hz: number): void => onOffset(clampOffsetHz(offsetHz + hz, limitHz));
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1">
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
      {centerHz !== null && Number.isFinite(centerHz) && (
        <Popover label="Freq" triggerClass={`${BTN} shrink-0`} width="w-64" align="end">
          {(close) => (
            <FrequencyForm
              centerHz={centerHz}
              offsetHz={offsetHz}
              limitHz={limitHz}
              onOffset={(hz) => {
                onOffset(hz);
                close();
              }}
            />
          )}
        </Popover>
      )}
    </span>
  );
}

function FrequencyForm({
  centerHz,
  offsetHz,
  limitHz,
  onOffset,
}: {
  centerHz: number;
  offsetHz: number;
  limitHz: number | null;
  onOffset: (hz: number) => void;
}) {
  const [text, setText] = useState(`${(centerHz + offsetHz) / 1e6}`);
  const frequencyHz = parseFrequencyHz(text);
  const offset = frequencyHz === null ? null : offsetForFrequencyHz(frequencyHz, centerHz, limitHz);
  return (
    <Form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (offset !== null) {
          onOffset(offset);
        }
      }}
    >
      <span className={LABEL}>Frequency (MHz)</span>
      <span className="flex items-center gap-2">
        <Input
          className={`${FIELD} min-w-0 flex-1 tabular-nums ${offset === null && text.trim() !== "" ? "border-danger" : ""}`}
          value={text}
          inputMode="decimal"
          autoFocus
          aria-label="Frequency to tune to"
          aria-invalid={offset === null}
          onChange={(event) => setText(event.target.value)}
        />
        <Button type="submit" className={BTN_PRIMARY} disabled={offset === null}>
          Set
        </Button>
      </span>
      <span className="legend">
        {limitHz === null
          ? `Center ${formatMhz(centerHz)}`
          : `Reaches ${formatMhz(centerHz - limitHz)} – ${formatMhz(centerHz + limitHz)}`}
      </span>
    </Form>
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
