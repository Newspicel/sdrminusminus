import { Combobox } from "@base-ui/react/combobox";
import { Check, ChevronDown } from "lucide-react";
import { FIELD, type Options, SURFACE, segment } from "./controls";
import { Icon } from "./Icon";
import { usePortalContainer } from "./PortalContainer";
import { type Choice, optionMatches } from "./selectFilter";

const TRIGGER = "w-full max-w-52";

export function SearchableSelect<T extends string | number>({
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
  const items = options.map((option) => ({
    value: option.value,
    label: option.label ?? String(option.value),
  }));
  const selected = items.find((item) => item.value === value) ?? null;

  return (
    <Combobox.Root
      items={items}
      value={selected}
      disabled={disabled}
      isItemEqualToValue={(item: Choice<T>, against: Choice<T> | null) =>
        item.value === against?.value
      }
      filter={(item: Choice<T>, query: string) => optionMatches(item, query)}
      onValueChange={(next: Choice<T> | null) => {
        if (next !== null) {
          onChange(next.value);
        }
      }}
    >
      <Combobox.Trigger
        data-hotkeys="off"
        aria-label={label}
        className={`${FIELD} justify-between ${className}`}
      >
        <span className="truncate">
          <Combobox.Value />
        </span>
        <Combobox.Icon aria-hidden className="shrink-0 text-ink-faint">
          <Icon glyph={ChevronDown} size={12} />
        </Combobox.Icon>
      </Combobox.Trigger>
      <Combobox.Portal container={portalContainer} className="contents">
        <Combobox.Positioner className="z-30" side="bottom" align="start" sideOffset={4}>
          <Combobox.Popup
            data-hotkeys="off"
            aria-label={label}
            className={`${SURFACE} flex max-h-[min(20rem,var(--available-height))] max-w-[calc(100vw-1rem)] min-w-[var(--anchor-width)] flex-col`}
          >
            <Combobox.Input placeholder={`Search ${label}`} className={`${FIELD} m-0.5 shrink-0`} />
            <Combobox.Empty className="px-2 py-3 text-sm text-ink-dim empty:hidden">
              Nothing matches that.
            </Combobox.Empty>
            <Combobox.List className="flex min-h-0 flex-col overflow-y-auto overscroll-contain p-0.5">
              {(item: Choice<T>) => (
                <Combobox.Item
                  key={String(item.value)}
                  value={item}
                  className={(state) =>
                    `${segment(state.selected)} grid grid-cols-[0.75rem_minmax(0,1fr)] items-center gap-2 text-left ${
                      state.highlighted && !state.selected ? "bg-panel-2 text-ink" : ""
                    }`
                  }
                >
                  <Combobox.ItemIndicator className="col-start-1">
                    <Icon glyph={Check} size={12} />
                  </Combobox.ItemIndicator>
                  <span className="col-start-2 truncate">{item.label}</span>
                </Combobox.Item>
              )}
            </Combobox.List>
          </Combobox.Popup>
        </Combobox.Positioner>
      </Combobox.Portal>
    </Combobox.Root>
  );
}
