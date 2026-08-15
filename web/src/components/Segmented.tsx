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
      data-hotkeys="off"
      aria-label={label}
      className="flex overflow-hidden rounded-[3px] border border-line"
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
