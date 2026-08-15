import type { Options } from "./controls";

export function withCurrent<T extends string | number>(
  value: T,
  options: Options<T>,
  format: (value: T) => string,
): Options<T> {
  return options.some((option) => option.value === value)
    ? options
    : [{ value, label: `${format(value)} (current)` }, ...options];
}
