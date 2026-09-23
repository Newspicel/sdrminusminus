import { Slider as Primitive } from "@base-ui/react/slider";
import type { ReactNode } from "react";

export function Slider({
  label,
  value,
  min,
  max,
  step,
  onChange,
  onCommit,
  className,
  disabled = false,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  onChange: (value: number) => void;
  onCommit?: (value: number) => void;
  className?: string;
}) {
  return (
    <Primitive.Root
      data-hotkeys="off"
      thumbAlignment="edge"
      className={`flex ${className ?? "w-24"} ${disabled ? "opacity-45" : ""}`}
      disabled={disabled}
      value={value}
      min={min}
      max={max}
      step={step}
      onValueChange={(next) => {
        if (typeof next === "number") {
          onChange(next);
        }
      }}
      onValueCommitted={(next) => {
        if (typeof next === "number") {
          onCommit?.(next);
        }
      }}
    >
      <Primitive.Control className="flex h-7 w-full cursor-pointer touch-none items-center data-disabled:cursor-default data-dragging:cursor-grabbing pointer-coarse:h-10">
        <Primitive.Track className="h-1 w-full rounded-full bg-well shadow-[inset_0_0_0_1px_var(--color-line)]">
          <Primitive.Indicator className="rounded-full bg-accent" />
          <Primitive.Thumb
            aria-label={label}
            className="size-3.5 cursor-grab rounded-full border border-accent bg-ink shadow-raised data-dragging:cursor-grabbing has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent has-[:focus-visible]:outline-offset-2"
          />
        </Primitive.Track>
      </Primitive.Control>
    </Primitive.Root>
  );
}

export function SliderField({
  label,
  value,
  min,
  max,
  step,
  readout,
  disabled = false,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  readout: ReactNode;
  disabled?: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <span className="flex min-w-0 flex-1 items-center gap-3">
      <Slider
        label={label}
        className="min-w-0 flex-1"
        disabled={disabled}
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={onChange}
      />
      <span
        className={`w-14 shrink-0 text-right font-mono text-xs tabular-nums ${
          disabled ? "text-ink-faint opacity-45" : "text-ink"
        }`}
      >
        {readout}
      </span>
    </span>
  );
}
