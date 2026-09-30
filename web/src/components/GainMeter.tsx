import { Slider as Primitive } from "@base-ui/react/slider";
import { formatPeak, HEADROOM, type MeterTone, meterUnit } from "./dbfs";

const TONE: Record<MeterTone, string> = {
  ok: "bg-ok",
  warn: "bg-warn",
  danger: "bg-danger",
};

const HEADROOM_TINT =
  `linear-gradient(to right, transparent ${HEADROOM.warn * 100}%, ` +
  `color-mix(in oklab, var(--color-warn) 22%, transparent) ${HEADROOM.warn * 100}% ${HEADROOM.hot * 100}%, ` +
  `color-mix(in oklab, var(--color-danger) 26%, transparent) ${HEADROOM.hot * 100}%)`;

const THUMB =
  "h-4 w-1 cursor-grab rounded-[2px] shadow-[0_0_0_1.5px_var(--color-panel)] " +
  "data-dragging:cursor-grabbing has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent " +
  "has-[:focus-visible]:outline-offset-2";

export function GainMeter({
  label,
  value,
  min,
  max,
  step,
  peakDb,
  tone = "ok",
  auto = false,
  disabled = false,
  onChange,
  onCommit,
  className,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  peakDb?: number | null;
  tone?: MeterTone;
  auto?: boolean;
  disabled?: boolean;
  onChange: (value: number) => void;
  onCommit?: (value: number) => void;
  className?: string;
}) {
  const metered = peakDb !== undefined;
  const level = metered ? meterUnit(peakDb ?? undefined) : 0;
  return (
    <Primitive.Root
      data-hotkeys="off"
      thumbAlignment="edge"
      className={`flex ${className ?? "w-24"}`}
      value={value}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
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
      <Primitive.Control
        className="flex h-7 w-full cursor-pointer touch-none items-center data-dragging:cursor-grabbing data-disabled:cursor-not-allowed data-disabled:opacity-50 pointer-coarse:h-10"
        title={
          metered
            ? `Handle: gain. Fill: signal, ${formatPeak(peakDb ?? undefined)}. Keep it out of the red`
            : undefined
        }
      >
        <Primitive.Track
          className="relative h-2 w-full rounded-[2px] bg-well shadow-[inset_0_0_0_1px_var(--color-line),inset_0_1px_2px_oklch(0_0_0/0.2)]"
          style={metered ? { backgroundImage: HEADROOM_TINT } : undefined}
        >
          {metered ? (
            <span
              aria-hidden
              className={`absolute inset-y-px left-px rounded-[1px] ${TONE[tone]}`}
              style={{ width: `max(0px, calc(${level * 100}% - 2px))` }}
            />
          ) : (
            <Primitive.Indicator className="rounded-[1px] bg-port-tx/60" />
          )}
          <Primitive.Thumb
            aria-label={label}
            className={`${THUMB} ${
              auto
                ? "bg-panel shadow-[inset_0_0_0_1.5px_var(--color-ink-dim),0_0_0_1.5px_var(--color-panel)] hover:shadow-[inset_0_0_0_1.5px_var(--color-accent),0_0_0_1.5px_var(--color-panel)]"
                : "bg-ink hover:bg-accent"
            }`}
          />
        </Primitive.Track>
      </Primitive.Control>
    </Primitive.Root>
  );
}
