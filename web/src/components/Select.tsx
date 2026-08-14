// The one-of-many control (). A native `<select>` draws its list in OS chrome, which
// ignores the palette and cannot carry the silkscreen voice; this renders the list in the same
// elevation-3 surface every other menu uses, from the same `segment()` parts.
import { Select as Primitive } from "@base-ui/react/select";
import { FIELD, type Options, SURFACE, segment } from "./controls";

/** A device — or a preset — can hold a value that is not one of the discrete points its
 * capabilities declare. Offering it as its own item is what keeps the list honest: without it
 * the trigger shows the first option, which is a lie about what the radio is set to. */
export function withCurrent<T extends string | number>(
  value: T,
  options: Options<T>,
  format: (value: T) => string,
): Options<T> {
  return options.some((option) => option.value === value)
    ? options
    : [{ value, label: `${format(value)} (current)` }, ...options];
}

export function Select<T extends string | number>({
  label,
  value,
  options,
  onChange,
  className,
}: {
  label: string;
  value: T;
  options: Options<T>;
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <Primitive.Root
      items={options}
      value={value}
      // Matching the option back by value keeps the call site's literal union; the change
      // callback is typed against the widened item value.
      onValueChange={(next) => {
        const picked = options.find((option) => option.value === next);
        if (picked !== undefined) {
          onChange(picked.value);
        }
      }}
    >
      <Primitive.Trigger
        // The list owns the arrows and the typeahead letters; without this the tuning layer
        // would act on them too (`useHotkeys`).
        data-hotkeys="off"
        aria-label={label}
        className={`${FIELD} justify-between ${className ?? ""}`}
      >
        <Primitive.Value className="truncate" />
        <Primitive.Icon aria-hidden className="shrink-0 text-ink-faint">
          ▾
        </Primitive.Icon>
      </Primitive.Trigger>
      <Primitive.Portal>
        {/* A drop-down, not the macOS overlay Base UI defaults to: a list that opens on top of
            its own trigger hides the value being changed. */}
        <Primitive.Positioner
          className="z-30"
          alignItemWithTrigger={false}
          side="bottom"
          align="start"
          sideOffset={4}
        >
          <Primitive.Popup
            data-hotkeys="off"
            className={`${SURFACE} max-w-[calc(100vw-1rem)] min-w-[var(--anchor-width)]`}
          >
            {/* A column flex, not `w-full` items: the positioner is shrink-to-fit, and a
                percentage width inside one resolves against the whole viewport. */}
            <Primitive.List className="flex max-h-[var(--available-height)] flex-col overflow-y-auto p-0.5">
              {options.map((option) => (
                <Primitive.Item
                  key={String(option.value)}
                  value={option.value}
                  className={(state) =>
                    `${segment(state.selected)} justify-start ${
                      state.highlighted && !state.selected ? "bg-panel-2 text-ink" : ""
                    }`
                  }
                >
                  <Primitive.ItemText>{option.label}</Primitive.ItemText>
                </Primitive.Item>
              ))}
            </Primitive.List>
          </Primitive.Popup>
        </Primitive.Positioner>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
