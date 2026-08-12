// Continuous settings that are swept rather than typed: gain, squelch, volume. The visual is one
// well and one accent thumb — the value itself is always printed beside the control, because a
// position on a 96px track is not a reading.
import { Slider as Primitive } from "@base-ui/react/slider";

export function Slider({
  label,
  value,
  min,
  max,
  step,
  onChange,
  onCommit,
  className,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number) => void;
  /** Fired once on release instead of per pixel, for a sweep whose every intermediate value
   * would be a real request — a playback seek, not a gain the DSP can take continuously. */
  onCommit?: (value: number) => void;
  /** Width is the caller's: a settings row wants the column, a channel header wants 80px. */
  className?: string;
}) {
  return (
    <Primitive.Root
      // The slider owns the arrows, Home and End; without this the tuning layer would act on
      // them too (`useHotkeys`).
      data-hotkeys="off"
      className={`flex ${className ?? "w-24"}`}
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
      {/* The control, not the track, carries the hit area: DESIGN.md §4's 40px coarse-pointer
          floor is bought with padding around a track that stays 6px. */}
      {/* The thumb is a `<div>`, so nothing gives it a cursor for free. `data-dragging` on the
          control, not only the thumb: a sweep that outruns the pointer leaves it over bare
          track, and the grip must not let go visually while the value is still moving. */}
      <Primitive.Control className="flex h-7 w-full cursor-pointer touch-none items-center data-dragging:cursor-grabbing pointer-coarse:h-10">
        <Primitive.Track className="h-1.5 w-full rounded-full bg-panel-2">
          <Primitive.Indicator className="rounded-full bg-accent-dim" />
          <Primitive.Thumb
            aria-label={label}
            className="size-3.5 cursor-grab rounded-full border border-line-strong bg-accent data-dragging:cursor-grabbing has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent has-[:focus-visible]:outline-offset-2"
          />
        </Primitive.Track>
      </Primitive.Control>
    </Primitive.Root>
  );
}
