import type { Binding, BindingGroup } from "../canvas/useHotkeys";

export const DOCS = "https://sdrmm.com/docs";
export const DISCORD = "https://discord.gg/dYaRyGwBNw";

export function docsPage(path: string): string {
  return `${DOCS}/${path}`;
}

export function host(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

export interface KeyPart {
  text: string;
  cap: boolean;
}

const SEPARATORS = new Set(["/", "–"]);

export function keyCaps(keys: string, apple: boolean): KeyPart[] {
  const local = keys.replace("Ctrl / ⌘", apple ? "⌘" : "Ctrl");
  return local
    .split(" ")
    .filter((text) => text.length > 0)
    .map((text) => ({ text, cap: !SEPARATORS.has(text) }));
}

export function isApple(userAgent: string): boolean {
  return /Mac|iPhone|iPad/.test(userAgent);
}

export function groupBindings(
  bindings: readonly Binding[],
): { group: BindingGroup; bindings: Binding[] }[] {
  const groups = new Map<BindingGroup, Binding[]>();
  for (const binding of bindings) {
    const list = groups.get(binding.group) ?? [];
    list.push(binding);
    groups.set(binding.group, list);
  }
  return [...groups].map(([group, list]) => ({ group, bindings: list }));
}
