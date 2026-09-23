import { NumberField as Primitive } from "@base-ui/react/number-field";
import { useState } from "react";
import { CONTROL_W, FIELD } from "./controls";
import { fractionDigits } from "./format";

interface Common {
  label: string;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  invalid?: boolean;
  disabled?: boolean;
  unit?: string;
}

export function NumberField({
  label,
  value,
  onCommit,
  min,
  max,
  step,
  className,
  invalid,
  disabled,
  unit,
}: Common & { value: number; onCommit: (value: number) => void }) {
  const [draft, setDraft] = useDraft(value);
  return (
    <Field
      label={label}
      min={min}
      max={max}
      step={step}
      className={className}
      invalid={invalid}
      disabled={disabled}
      unit={unit}
      value={draft}
      onDraft={setDraft}
      onCommit={(committed) => {
        if (committed === null) {
          setDraft(value);
        } else if (committed !== value) {
          onCommit(committed);
        }
      }}
      onRevert={() => setDraft(value)}
    />
  );
}

export function OptionalNumberField({
  label,
  placeholder,
  value,
  onCommit,
  min,
  max,
  step,
  className,
  invalid,
  unit,
}: Common & {
  placeholder: string;
  value: number | null;
  onCommit: (value: number | null) => void;
}) {
  const [draft, setDraft] = useDraft(value);
  return (
    <Field
      label={label}
      placeholder={placeholder}
      min={min}
      max={max}
      step={step}
      className={className}
      invalid={invalid}
      unit={unit}
      value={draft}
      onDraft={setDraft}
      onCommit={(committed) => {
        if (committed !== value) {
          onCommit(committed);
        }
      }}
      onRevert={() => setDraft(value)}
    />
  );
}

function Field({
  label,
  placeholder,
  min,
  max,
  step,
  className,
  invalid,
  disabled,
  unit,
  value,
  onDraft,
  onCommit,
  onRevert,
}: Common & {
  placeholder?: string;
  value: number | null;
  onDraft: (value: number | null) => void;
  onCommit: (value: number | null) => void;
  onRevert: () => void;
}) {
  const width = unit === undefined ? (className ?? CONTROL_W) : "min-w-0 flex-1";
  const field = (
    <Primitive.Root
      className={width}
      value={value}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      onValueChange={onDraft}
      onValueCommitted={onCommit}
      format={{ useGrouping: false, maximumFractionDigits: fractionDigits(step) }}
    >
      <Primitive.Input
        aria-label={label}
        aria-invalid={invalid}
        placeholder={placeholder}
        className={`${FIELD} w-full tabular-nums ${invalid === true ? "border-danger" : ""}`}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            onRevert();
          }
          if (event.key === "Enter") {
            onCommit(value);
          }
        }}
      />
    </Primitive.Root>
  );
  if (unit === undefined) {
    return field;
  }
  return (
    <span className={`flex items-center gap-1.5 ${className ?? CONTROL_W}`}>
      {field}
      <span className="legend w-8 shrink-0">{unit}</span>
    </span>
  );
}

function useDraft(value: number | null): [number | null, (next: number | null) => void] {
  const [draft, setDraft] = useState(value);
  const [seen, setSeen] = useState(value);
  if (seen !== value) {
    setSeen(value);
    setDraft(value);
    return [value, setDraft];
  }
  return [draft, setDraft];
}
