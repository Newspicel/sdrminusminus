// Shared control looks for the instrument strips (PLAN §10) — one place so every panel's
// buttons and fields stay identical. `max-md:min-h-10` keeps controls at a ≥40px touch
// target on phones.
export const FIELD =
  "rounded border border-line bg-panel-2 px-2 py-1 font-mono text-ink max-md:min-h-10";

export const BTN =
  "rounded border border-line bg-panel-2 px-2.5 py-1 text-sm text-ink transition-colors " +
  "hover:border-accent hover:text-accent disabled:opacity-40 max-md:min-h-10";
