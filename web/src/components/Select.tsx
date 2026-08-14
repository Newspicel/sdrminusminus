import {
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Select as ShadcnSelect,
} from "@/components/ui/select";
import { type Options } from "./controls";
import { usePortalContainer } from "./PortalContainer";

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
  const portalContainer = usePortalContainer();
  return (
    <ShadcnSelect
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
      <SelectTrigger
        // The list owns the arrows and the typeahead letters; without this the tuning layer
        // would act on them too (`useHotkeys`).
        data-hotkeys="off"
        aria-label={label}
        size="sm"
        className={className}
      >
        <SelectValue className="truncate" />
      </SelectTrigger>
      <SelectContent
        container={portalContainer}
        data-hotkeys="off"
        alignItemWithTrigger={false}
        align="start"
      >
        {options.map((option) => (
          <SelectItem key={String(option.value)} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </ShadcnSelect>
  );
}
