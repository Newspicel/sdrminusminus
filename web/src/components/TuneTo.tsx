import { useState } from "react";
import { Button, Form, Input } from "./BaseControls";
import { BTN_PRIMARY, FIELD, ICON_BTN, LABEL } from "./controls";
import { parseFrequency } from "./dial";
import { Popover } from "./Popover";

const KEYPAD_DOTS = [4, 8, 12].flatMap((cy) => [4, 8, 12].map((cx) => ({ cx, cy })));

function KeypadGlyph() {
  return (
    <svg viewBox="0 0 16 16" className="size-4" fill="currentColor" aria-hidden>
      {KEYPAD_DOTS.map((dot) => (
        <circle key={`${dot.cx}:${dot.cy}`} cx={dot.cx} cy={dot.cy} r="1.1" />
      ))}
    </svg>
  );
}

export function TuneTo({
  title,
  hz,
  hint,
  resolve,
  disabled = false,
  onTune,
}: {
  title: string;
  hz: number;
  hint: string;
  resolve: (frequencyHz: number) => number | null;
  disabled?: boolean;
  onTune: (value: number) => void;
}) {
  return (
    <Popover
      label={<KeypadGlyph />}
      title={title}
      triggerClass={`${ICON_BTN} shrink-0`}
      width="w-64"
      align="end"
      disabled={disabled}
    >
      {(close) => (
        <TuneForm
          hz={hz}
          hint={hint}
          resolve={resolve}
          onTune={(value) => {
            onTune(value);
            close();
          }}
        />
      )}
    </Popover>
  );
}

function TuneForm({
  hz,
  hint,
  resolve,
  onTune,
}: {
  hz: number;
  hint: string;
  resolve: (frequencyHz: number) => number | null;
  onTune: (value: number) => void;
}) {
  const [text, setText] = useState(`${hz / 1e6}`);
  const entered = parseFrequency(text);
  const target = entered === null ? null : resolve(entered);
  return (
    <Form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (target !== null) {
          onTune(target);
        }
      }}
    >
      <span className={LABEL}>Frequency (MHz)</span>
      <span className="flex items-center gap-2">
        <Input
          className={`${FIELD} min-w-0 flex-1 tabular-nums ${target === null && text.trim() !== "" ? "border-danger" : ""}`}
          value={text}
          inputMode="decimal"
          autoFocus
          aria-label="Frequency to tune to"
          aria-invalid={target === null}
          onChange={(event) => setText(event.target.value)}
        />
        <Button type="submit" className={BTN_PRIMARY} disabled={target === null}>
          Set
        </Button>
      </span>
      <span className="legend">{hint}</span>
    </Form>
  );
}
