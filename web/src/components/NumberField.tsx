// Numeric input that keeps a local buffer while typing and commits on blur/Enter — per-keystroke
// PATCHes would flood the server and fight the WS-refreshed value. Commits clamp to the declared
// range.
import { useState } from "react";
import { FIELD } from "./controls";

export function NumberField({
  label,
  value,
  onCommit,
  min,
  max,
  step,
  className,
}: {
  label: string;
  value: number;
  onCommit: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
}) {
  const [text, setText] = useState<string | null>(null);

  const commit = (): void => {
    if (text === null) {
      return;
    }
    setText(null);
    if (text.trim() === "") {
      return;
    }
    const entered = Number(text);
    if (!Number.isFinite(entered)) {
      return;
    }
    const clamped = Math.min(
      max ?? Number.POSITIVE_INFINITY,
      Math.max(min ?? Number.NEGATIVE_INFINITY, entered),
    );
    if (clamped !== value) {
      onCommit(clamped);
    }
  };

  return (
    <input
      type="number"
      inputMode="decimal"
      className={`${FIELD} tabular-nums ${className ?? "w-20"}`}
      aria-label={label}
      value={text ?? String(value)}
      min={min}
      max={max}
      step={step}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          commit();
        } else if (e.key === "Escape") {
          setText(null);
        }
      }}
    />
  );
}
