// Theme selection (). Unlike workspaces, a theme is a property of the eye looking
// at the screen rather than of the workspace, so it lives in `localStorage` and never syncs
// between clients.
//
// The document only ever carries a *resolved* theme (`data-theme="dark" | "light"`), so the
// stylesheet needs no `prefers-color-scheme` branch and canvases can read the resolved tokens
// with one `getComputedStyle`.
import { useSyncExternalStore } from "react";

export type ThemeChoice = "system" | "dark" | "light";
export type ResolvedTheme = "dark" | "light";

/** The order the one theme control walks. Auto first because it is where an install starts, then
 * the two explicit choices in the order a night bench reaches for them. */
export const THEME_CYCLE: readonly ThemeChoice[] = ["system", "dark", "light"];

/** The choice one press of the theme control moves to. */
export function nextTheme(choice: ThemeChoice): ThemeChoice {
  const at = THEME_CYCLE.indexOf(choice);
  return THEME_CYCLE[(at + 1) % THEME_CYCLE.length] ?? "system";
}

export interface ThemeState {
  choice: ThemeChoice;
  resolved: ResolvedTheme;
}

const KEY = "sdrmm.theme";
const LIGHT = "(prefers-color-scheme: light)";
const DEFAULT: ThemeState = { choice: "system", resolved: "dark" };

const listeners = new Set<() => void>();
// `useSyncExternalStore` compares snapshots by identity, so this is rebuilt only on a real
// change. Nothing touches the DOM until `initTheme` runs, which keeps the module importable
// from a non-browser test.
let state: ThemeState = DEFAULT;

/** Applied before React mounts so the first paint is already in the right theme. */
export function initTheme(): void {
  state = { choice: read(), resolved: DEFAULT.resolved };
  apply();
  window.matchMedia(LIGHT).addEventListener("change", () => {
    if (state.choice === "system") {
      apply();
    }
  });
}

export function setTheme(next: ThemeChoice): void {
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // A blocked or full store costs the preference on the next load, not this session.
  }
  state = { ...state, choice: next };
  apply();
}

export function useTheme(): ThemeState {
  return useSyncExternalStore(onThemeChange, snapshot, serverSnapshot);
}

/** Fires whenever the resolved theme changes — how canvases pick up new token values without
 * re-reading `getComputedStyle` every frame. */
export function onThemeChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function apply(): void {
  const resolved: ResolvedTheme =
    state.choice === "system"
      ? window.matchMedia(LIGHT).matches
        ? "light"
        : "dark"
      : state.choice;
  document.documentElement.dataset.theme = resolved;
  state = { choice: state.choice, resolved };
  for (const listener of listeners) {
    listener();
  }
}

function read(): ThemeChoice {
  try {
    const stored = localStorage.getItem(KEY);
    return stored === "dark" || stored === "light" || stored === "system" ? stored : "system";
  } catch {
    return "system";
  }
}

function snapshot(): ThemeState {
  return state;
}

function serverSnapshot(): ThemeState {
  return DEFAULT;
}
