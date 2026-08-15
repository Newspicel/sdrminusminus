// Option-list shaping for `Select`. Kept out of `Select.tsx` so that file exports only
// components — a mixed module costs Fast Refresh the component state it would otherwise preserve.
import type { Options } from "./controls";

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
