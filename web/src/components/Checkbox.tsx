import { Checkbox as Primitive } from "@base-ui/react/checkbox";

export function Checkbox({
  label,
  checked,
  disabled,
  onChange,
}: {
  label?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <Primitive.Root
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onCheckedChange={onChange}
      className="relative flex size-4 shrink-0 items-center justify-center rounded-[3px] border border-line-strong bg-panel-2 text-bg data-checked:border-accent data-checked:bg-accent data-disabled:opacity-45 pointer-coarse:before:absolute pointer-coarse:before:-inset-3"
    >
      <Primitive.Indicator className="flex data-unchecked:hidden">
        <svg viewBox="0 0 16 16" className="size-3" fill="none" stroke="currentColor" aria-hidden>
          <path d="m2.5 8.5 4 4 7-9" strokeWidth="2" />
        </svg>
      </Primitive.Indicator>
    </Primitive.Root>
  );
}
