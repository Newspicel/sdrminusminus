import { onThemeChange } from "./theme";

const cache = new Map<string, string>();
let subscribed = false;

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
