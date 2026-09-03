const INTERACTIVE =
  "inline-flex items-center gap-1.5 rounded-[3px] transition-colors duration-100 " +
  "disabled:opacity-45 disabled:pointer-events-none pointer-coarse:min-h-10";

export const BTN =
  `${INTERACTIVE} h-7 border border-line-strong bg-panel-2 px-2.5 text-xs text-ink ` +
  "hover:border-accent hover:text-accent";

export const BTN_SM =
  `${INTERACTIVE} h-5 border border-line-strong bg-panel-2 px-1.5 font-mono text-[10px] ` +
  "tracking-[0.09em] uppercase text-ink-dim hover:border-accent hover:text-accent";

export const BTN_PRIMARY =
  `${INTERACTIVE} h-7 border border-accent bg-accent px-3 text-xs font-medium text-bg ` +
  "hover:brightness-110";

export const BTN_QUIET =
  `${INTERACTIVE} h-7 border border-transparent px-2 text-xs text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink";

export const BTN_DANGER =
  `${INTERACTIVE} h-7 border border-line-strong bg-panel-2 px-2.5 text-xs text-ink ` +
  "hover:border-danger hover:bg-danger/10 hover:text-danger";

export const BTN_DANGER_SM =
  `${INTERACTIVE} h-5 border border-danger bg-danger/10 px-1.5 font-mono text-[10px] ` +
  "tracking-[0.09em] uppercase text-danger hover:bg-danger/20";

const ICON_BASE =
  `${INTERACTIVE} justify-center border border-transparent text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink";

export const ICON_BTN = `${ICON_BASE} size-7 pointer-coarse:size-10`;

export const ICON_BTN_SM = `${ICON_BASE} size-5 pointer-coarse:size-10`;

export const FIELD =
  `${INTERACTIVE} h-7 min-w-0 border border-line-strong bg-panel-2 px-2 font-mono text-xs ` +
  "text-ink placeholder:text-ink-faint hover:border-accent-dim";

export const LABEL =
  "inline-flex items-center gap-2 whitespace-nowrap font-mono text-[10px] tracking-[0.09em] " +
  "uppercase text-ink-faint";

export const CHIP =
  "inline-flex h-7 items-center gap-1.5 rounded-[3px] border border-line bg-panel-2 px-2 " +
  "font-mono text-xs text-ink";

export const SURFACE = "rounded-md border border-line-strong bg-panel-3 shadow-pop";

export const ALERT =
  "rounded-[3px] border border-danger bg-danger/10 px-3 py-1.5 font-mono text-xs text-danger";

export const TABLE_HEAD =
  "px-2 py-1 text-left font-mono text-[10px] tracking-[0.09em] uppercase text-ink-dim";
export const TABLE_CELL = "px-2 py-1 align-top font-mono text-xs tabular-nums";

export function plotButton(on: boolean): string {
  return (
    `${INTERACTIVE} h-6 border border-transparent px-1.5 font-mono text-[10px] tracking-[0.09em] uppercase ` +
    (on
      ? "bg-plot-ink/15 text-plot-ink"
      : "text-plot-ink-dim hover:bg-plot-ink/12 hover:text-plot-ink")
  );
}

export function segment(selected: boolean): string {
  return (
    `${INTERACTIVE} h-7 px-2.5 text-xs ` +
    (selected
      ? "bg-accent/15 font-medium text-accent"
      : "text-ink-dim hover:bg-panel-2 hover:text-ink")
  );
}

export function segmentSm(selected: boolean): string {
  return (
    `${INTERACTIVE} h-5 px-1.5 font-mono text-[10px] tracking-[0.09em] uppercase ` +
    (selected
      ? "bg-accent/15 font-medium text-accent"
      : "text-ink-faint hover:bg-panel-2 hover:text-ink")
  );
}

export function commitText(
  candidate: string,
  value: string,
  onCommit: (value: string) => boolean,
): string {
  const next = candidate.trim();
  return next !== value && !onCommit(next) ? value : next;
}

export type Options<T> = readonly { value: T; label: string }[];
