import { Toggle } from "@base-ui/react/toggle";
import { ToggleGroup } from "@base-ui/react/toggle-group";
import { type Options, segment } from "./controls";

export function Segmented<T extends string | number>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: Options<T>;
  onChange: (value: T) => void;
}) {
  return (
    <ToggleGroup
      // Arrow keys walk the members; without this the tuning layer would act on them too
      // (`useHotkeys`).
      data-hotkeys="off"
      aria-label={label}
      className="flex overflow-hidden rounded-[3px] border border-line"
      // Stringified because the group's value is a string list. A value that is not on the list
      // (a preset field holding a typed-in number) simply shows nothing pressed, and pressing
      // the pressed member yields an empty list, which falls through as "no change" — a
      // segmented control has no off state.
      value={[String(value)]}
      onValueChange={(next) => {
        const picked = options.find((option) => String(option.value) === next[0]);
        if (picked !== undefined) {
          onChange(picked.value);
        }
      }}
    >
      {options.map((option) => (
        <Toggle
          key={String(option.value)}
          value={String(option.value)}
          className={(state) => `${segment(state.pressed)} rounded-none font-mono tabular-nums`}
        >
          {option.label}
        </Toggle>
      ))}
    </ToggleGroup>
  );
}
