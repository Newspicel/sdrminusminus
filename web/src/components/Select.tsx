import { Select as Primitive } from "@base-ui/react/select";
import { FIELD, type Options, SURFACE, segment } from "./controls";
import { usePortalContainer } from "./PortalContainer";

const TRIGGER = "w-full max-w-52";

export function Select<T extends string | number>({
  label,
  value,
  options,
  onChange,
  className = TRIGGER,
  disabled = false,
}: {
  label: string;
  value: T;
  options: Options<T>;
  onChange: (value: T) => void;
  className?: string;
  disabled?: boolean;
}) {
  const portalContainer = usePortalContainer();
  return (
    <Primitive.Root
      items={options}
      value={value}
      disabled={disabled}
      onValueChange={(next) => {
        const picked = options.find((option) => option.value === next);
        if (picked !== undefined) {
          onChange(picked.value);
        }
      }}
    >
      <Primitive.Trigger
        data-hotkeys="off"
        aria-label={label}
        className={`${FIELD} justify-between ${className}`}
      >
        <Primitive.Value className="truncate" />
        <Primitive.Icon aria-hidden className="shrink-0 text-ink-faint">
          ▾
        </Primitive.Icon>
      </Primitive.Trigger>
      <Primitive.Portal container={portalContainer} className="contents">
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
