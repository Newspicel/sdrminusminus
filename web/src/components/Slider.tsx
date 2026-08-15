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
