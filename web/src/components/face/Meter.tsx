import { Slider as Primitive } from "@base-ui/react/slider";
import type { ReactNode } from "react";
import { PortAnchor } from "../../canvas/nodes/NodeShell";
import { formatPeak, HEADROOM, type MeterTone, meterUnit } from "../dbfs";

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

const THUMB_AUTO =
  "bg-panel shadow-[inset_0_0_0_1.5px_var(--color-ink-dim),0_0_0_1.5px_var(--color-panel)] " +
  "hover:shadow-[inset_0_0_0_1.5px_var(--color-accent),0_0_0_1.5px_var(--color-panel)]";

const ROW = "grid h-7 grid-cols-[3.5rem_minmax(0,1fr)_3.75rem_2.75rem_0] items-center gap-x-2";

export const METER_READOUT = "text-right font-mono text-xs tabular-nums whitespace-nowrap text-ink";

export interface MeterFill {
  level: number;
  peak?: number;
  className: string;
  tint?: string;
}

export function MeterSlider({
  label,
  value,
  min,
  max,
  step,
  fill,
  title,
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
  fill?: MeterFill;
  title?: string;
  auto?: boolean;
  disabled?: boolean;
  onChange: (value: number) => void;
  onCommit?: (value: number) => void;
  className?: string;
}) {
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
        title={title}
      >
        <Primitive.Track
          className="relative h-2 w-full rounded-[2px] bg-well shadow-[inset_0_0_0_1px_var(--color-line),inset_0_1px_2px_oklch(0_0_0/0.2)]"
          style={fill?.tint === undefined ? undefined : { backgroundImage: fill.tint }}
        >
          {fill === undefined ? (
            <Primitive.Indicator className="rounded-[1px] bg-port-tx/60" />
          ) : (
            <>
              <span
                aria-hidden
                className={`absolute inset-y-px left-px rounded-[1px] ${fill.className}`}
                style={{ width: `max(0px, calc(${fill.level * 100}% - 2px))` }}
              />
              {fill.peak !== undefined && fill.peak > 0 && (
                <span
                  aria-hidden
                  className="absolute inset-y-px w-px bg-ink-dim"
                  style={{ left: `calc(${fill.peak * 100}% - 1px)` }}
                />
              )}
            </>
          )}
          <Primitive.Thumb
            aria-label={label}
            className={`${THUMB} ${auto ? THUMB_AUTO : "bg-ink hover:bg-accent"}`}
          />
        </Primitive.Track>
      </Primitive.Control>
    </Primitive.Root>
  );
}

export function MeterBar({
  label,
  value,
  valueText,
  peak,
  fill = "bg-accent",
}: {
  label: string;
  value: number;
  valueText?: string;
  peak?: number;
  fill?: string;
}) {
  const unit = Math.min(1, Math.max(0, value));
  return (
    <span
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(unit * 100)}
      aria-valuetext={valueText}
      className="relative block h-2 w-full rounded-[2px] bg-well shadow-[inset_0_0_0_1px_var(--color-line),inset_0_1px_2px_oklch(0_0_0/0.2)]"
    >
      <span
        aria-hidden
        className={`absolute inset-y-px left-px rounded-[1px] ${fill}`}
        style={{ width: `max(0px, calc(${unit * 100}% - 2px))` }}
      />
      {peak !== undefined && peak > 0 && (
        <span
          aria-hidden
          className="absolute inset-y-px w-px bg-ink-dim"
          style={{ left: `calc(${Math.min(1, peak) * 100}% - 1px)` }}
        />
      )}
    </span>
  );
}

export function GainMeter({
  peakDb,
  tone = "ok",
  ...slider
}: Omit<Parameters<typeof MeterSlider>[0], "fill" | "title"> & {
  peakDb?: number | null;
  tone?: MeterTone;
}) {
  const metered = peakDb !== undefined;
  return (
    <MeterSlider
      {...slider}
      fill={
        metered
          ? { level: meterUnit(peakDb ?? undefined), className: TONE[tone], tint: HEADROOM_TINT }
          : undefined
      }
      title={
        metered
          ? `Handle: gain. Fill: signal, ${formatPeak(peakDb ?? undefined)}. Keep it out of the red`
          : undefined
      }
    />
  );
}

export function MeterRow({
  label,
  meter,
  readout,
  trailing,
  port,
  title,
}: {
  label: ReactNode;
  meter: ReactNode;
  readout?: ReactNode;
  trailing?: ReactNode;
  port?: string;
  title?: string;
}) {
  return (
    <div className={ROW} title={title}>
      {typeof label === "string" ? <span className="legend truncate">{label}</span> : label}
      {meter}
      {readout === undefined ? <span /> : <span className={METER_READOUT}>{readout}</span>}
      {trailing ?? <span />}
      {port !== undefined && <PortAnchor port={port} />}
    </div>
  );
}
