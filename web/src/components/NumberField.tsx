// Numeric input that keeps a local draft while typing and commits on blur/Enter — per-keystroke
// PATCHes would flood the server and fight the WS-refreshed value. Commits clamp to the declared
// range.

import { useRef, useState } from "react";
import { Input } from "@/components/ui/input";
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
  const skipBlurCommit = useRef(false);
  return (
    <Input
      type="number"
      value={value ?? ""}
      min={min}
      max={max}
      step={step}
      aria-label={label}
      aria-invalid={invalid}
      placeholder={placeholder}
      className={`font-mono tabular-nums ${className ?? "w-20"}`}
      onChange={(event) => onDraft(parseDraft(event.currentTarget.value))}
      onBlur={() => {
        if (skipBlurCommit.current) {
          skipBlurCommit.current = false;
          return;
        }
        onCommit(clamp(value, min, max, step));
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          skipBlurCommit.current = true;
          onCommit(clamp(value, min, max, step));
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          event.preventDefault();
          skipBlurCommit.current = true;
          onRevert();
          event.currentTarget.blur();
        }
      }}
    />
  );
}

function parseDraft(value: string): number | null {
  if (value.trim() === "") return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function clamp(value: number | null, min?: number, max?: number, step?: number): number | null {
  if (value === null) return null;
  const bounded = Math.min(
    max ?? Number.POSITIVE_INFINITY,
    Math.max(min ?? Number.NEGATIVE_INFINITY, value),
  );
  return Number(bounded.toFixed(fractionDigits(step)));
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
