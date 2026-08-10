// Canvas drawing needs the palette as strings, and `index.css` is the only place colour is
// decided (DESIGN.md §2). Reading the custom property keeps that true for the 2D contexts,
// which cannot use a class.
//
// Values are cached because `getComputedStyle` forces style resolution and the spectrum reads
// its palette on every frame; the cache is dropped when the theme changes.
import { onThemeChange } from "./theme";

const cache = new Map<string, string>();
let subscribed = false;

/** The resolved value of a `--color-*` token, e.g. `token("plot-trace")`. */
export function token(name: string): string {
  if (!subscribed) {
    subscribed = true;
    onThemeChange(() => cache.clear());
  }
  const hit = cache.get(name);
  if (hit !== undefined) {
    return hit;
  }
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue(`--color-${name}`)
    .trim();
  cache.set(name, value);
  return value;
}

/** The same token at a fraction of its opacity — for gridlines and fills, which must stay
 * lighter-weight than the data they sit behind. */
export function tokenAlpha(name: string, alpha: number): string {
  return `color-mix(in oklab, ${token(name)} ${Math.round(alpha * 100)}%, transparent)`;
}
