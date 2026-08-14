import { Checkbox as ShadcnCheckbox } from "@/components/ui/checkbox";

export function Checkbox({
  label,
  checked,
  onChange,
}: {
  /** Only when the caller has no visible `<label>` around the box — an accessible name from
   * both would be read twice. */
  label?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <ShadcnCheckbox
      // Space toggles the box; without this it would also start audio (`useHotkeys`).
      data-hotkeys="off"
      aria-label={label}
      checked={checked}
      onCheckedChange={onChange}
    />
  );
}
