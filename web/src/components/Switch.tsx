import { Switch as Primitive } from "@base-ui/react/switch";

export function Switch({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <Primitive.Root
      aria-label={label}
      checked={checked}
      onCheckedChange={onChange}
      className="relative flex h-4 w-7 shrink-0 cursor-pointer items-center rounded-full border border-line-strong bg-panel-2 p-px transition-colors duration-100 data-checked:border-accent data-checked:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent pointer-coarse:before:absolute pointer-coarse:before:-inset-3"
    >
      <Primitive.Thumb className="size-3 rounded-full bg-ink-dim transition-transform duration-100 data-checked:translate-x-3 data-checked:bg-bg" />
    </Primitive.Root>
  );
}
