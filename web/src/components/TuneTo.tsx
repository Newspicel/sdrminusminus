import { Keyboard } from "lucide-react";
import { useState } from "react";
import { Button, Form, Input } from "./BaseControls";
import { BTN_PRIMARY, FIELD, ICON_BTN, LABEL } from "./controls";
import { parseFrequency } from "./dial";
import { Icon } from "./Icon";
import { Popover } from "./Popover";
import { FieldUnitFrame, unitPadding } from "./Unit";

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
      label={<Icon glyph={Keyboard} size={16} />}
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
  const typedUnit = /[a-z]/i.test(text);
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
      <span className={LABEL}>Frequency</span>
      <span className="flex items-center gap-2">
        <FieldUnitFrame symbol={typedUnit ? "" : "MHz"} className="min-w-0 flex-1">
          <Input
            className={`${FIELD} w-full tabular-nums ${target === null && text.trim() !== "" ? "border-danger" : ""}`}
            style={typedUnit ? undefined : unitPadding("MHz")}
            value={text}
            inputMode="decimal"
            autoFocus
            aria-label="Frequency to tune to"
            aria-invalid={target === null}
            onChange={(event) => setText(event.target.value)}
            onFocus={(event) => event.currentTarget.select()}
          />
        </FieldUnitFrame>
        <Button type="submit" className={BTN_PRIMARY} disabled={target === null}>
          Set
        </Button>
      </span>
      <span className="legend">{hint}</span>
    </Form>
  );
}
