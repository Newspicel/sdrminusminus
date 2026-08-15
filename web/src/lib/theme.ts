import { useSyncExternalStore } from "react";

export type ThemeChoice = "system" | "dark" | "light";
export type ResolvedTheme = "dark" | "light";

export const THEME_CYCLE: readonly ThemeChoice[] = ["system", "dark", "light"];

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
let state: ThemeState = DEFAULT;

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
  } catch {}
  state = { ...state, choice: next };
  apply();
}

export function useTheme(): ThemeState {
  return useSyncExternalStore(onThemeChange, snapshot, serverSnapshot);
}

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
