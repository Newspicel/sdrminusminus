// Numeric input that keeps a local draft while typing and commits on blur/Enter — per-keystroke
// PATCHes would flood the server and fight the WS-refreshed value. Commits clamp to the declared
// range.
import { NumberField as Primitive } from "@base-ui/react/number-field";
import { useState } from "react";
import { FIELD } from "./controls";
import { fractionDigits } from "./format";

interface Common {
  label: string;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  /** For a field whose value is only wrong in company — see `AdsbReference`. A field that can
   * validate itself clamps instead, so this stays out of the common path. */
  invalid?: boolean;
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
      value={draft}
      onDraft={setDraft}
      onCommit={(committed) => {
        // An empty field is not a value for these settings, so it reads as an abandoned edit
        // rather than a rejected one: snap back to what the radio is actually set to.
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

/** For settings where an empty field is itself a value (auto-track, no reference position) —
 * otherwise there is no way back to auto once a number has been set. */
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
  return (
    <Primitive.Root
      value={value}
      min={min}
      max={max}
      step={step}
      // `onValueCommitted` is the contract: it fires on blur and on a released stepper, never
      // per keystroke. Enter is ours — the primitive treats it as a navigation key outside a
      // form, so a typed number would sit there uncommitted until focus left the field.
      onValueChange={onDraft}
      onValueCommitted={onCommit}
      format={{ useGrouping: false, maximumFractionDigits: fractionDigits(step) }}
    >
      <Primitive.Input
        aria-label={label}
        aria-invalid={invalid}
        placeholder={placeholder}
        className={`${FIELD} tabular-nums ${className ?? "w-20"} ${invalid === true ? "border-danger" : ""}`}
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
}

/** Local while the operator types, replaced whenever the setting itself changes — a WS refresh
 * or a server clamp is the truth, and re-seeding on it is what makes a rejected edit visible. */
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
