import { Checkbox as Primitive } from "@base-ui/react/checkbox";
import { Check } from "lucide-react";
import { Icon } from "./Icon";

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
        <Icon glyph={Check} size={12} />
      </Primitive.Indicator>
    </Primitive.Root>
  );
}
