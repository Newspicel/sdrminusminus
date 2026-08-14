// Continuous settings that are swept rather than typed: gain, squelch, volume. The visual is one
// well and one accent thumb — the value itself is always printed beside the control, because a
// position on a 96px track is not a reading.
import { Slider as ShadcnSlider } from "@/components/ui/slider";

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
    <ShadcnSlider
      // The slider owns the arrows, Home and End; without this the tuning layer would act on
      // them too (`useHotkeys`).
      data-hotkeys="off"
      aria-label={label}
      className={className ?? "w-24"}
      value={[value]}
      min={min}
      max={max}
      step={step}
      onValueChange={(next) => {
        const nextValue = Array.isArray(next) ? next[0] : next;
        if (nextValue !== undefined) {
          onChange(nextValue);
        }
      }}
      onValueCommitted={(next) => {
        const nextValue = Array.isArray(next) ? next[0] : next;
        if (nextValue !== undefined) {
          onCommit?.(nextValue);
        }
      }}
    />
  );
}
