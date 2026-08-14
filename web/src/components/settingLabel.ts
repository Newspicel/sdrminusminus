/** The display form of a device setting's name.
 *
 * Drivers name settings the way registers are named — `digital_agc`, `offset_tune` — and the
 * silkscreen voice uppercases them, so the raw key reaches a label track as one unbreakable run
 * that can only be truncated. Splitting on the separators gives the words back so they wrap
 * instead. The raw key stays on the row's `title`, because that is the string an operator
 * matches against a driver's documentation.
 */
export function settingLabel(name: string): string {
  return name
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .trim();
}
