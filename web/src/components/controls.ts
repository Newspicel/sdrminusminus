const INSTANT =
  "inline-flex items-center gap-1.5 rounded-[3px] " +
  "disabled:opacity-45 disabled:pointer-events-none pointer-coarse:min-h-10";

const INTERACTIVE = `${INSTANT} transition-colors duration-100`;

const RAISED = "bg-panel-2 text-ink";

export const BTN =
  `${INTERACTIVE} h-7 border border-line ${RAISED} px-2.5 text-xs font-medium ` +
  "hover:border-line-strong hover:bg-panel-3";

export const BTN_SM =
  `${INTERACTIVE} h-5 border border-line ${RAISED} px-1.5 text-[11px] font-medium ` +
  "hover:border-line-strong";

export const BTN_PRIMARY =
  `${INTERACTIVE} h-7 border border-accent bg-accent px-3 text-xs font-semibold text-on-accent ` +
  "shadow-raised hover:brightness-105";

export const BTN_QUIET =
  `${INTERACTIVE} h-7 border border-transparent px-2 text-xs text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink";

export const BTN_DANGER =
  `${INTERACTIVE} h-7 border border-line ${RAISED} px-2.5 text-xs font-medium ` +
  "hover:border-danger hover:bg-danger/10 hover:text-danger";

export const BTN_DANGER_SM =
  `${INTERACTIVE} h-5 border border-danger/60 bg-danger/10 px-1.5 text-[11px] font-medium ` +
  "text-danger hover:bg-danger/20";

const ICON_BASE =
  `${INTERACTIVE} justify-center border border-transparent text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink";

export const ICON_BTN = `${ICON_BASE} size-7 pointer-coarse:size-10`;

export const ICON_BTN_SM = `${ICON_BASE} size-5 pointer-coarse:size-10`;

export const FIELD =
  `${INTERACTIVE} h-7 min-w-0 border border-line bg-well px-2 font-mono text-xs ` +
  "text-ink placeholder:font-sans placeholder:text-ink-faint hover:border-line-strong " +
  "focus-visible:border-accent-dim focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/20";

export const CONTROL_W = "w-full max-w-52";

export const LABEL =
  "inline-flex items-center gap-2 whitespace-nowrap font-mono text-[10.5px] text-ink-faint";

export const CHIP =
  "inline-flex h-7 items-center gap-1.5 rounded-[3px] border border-line bg-well px-2 " +
  "font-mono text-xs text-ink";

export const CHIP_SM =
  "inline-flex h-5 items-center rounded-[3px] border border-line bg-well px-1.5 " +
  "font-mono text-[10px] text-ink";

export const SURFACE = "rounded-[3px] border border-line-strong/70 bg-panel-3 shadow-pop";

export const WELL = "flex gap-px rounded-[3px] border border-line bg-well p-px";

export const ALERT =
  "rounded-[3px] border border-danger/60 bg-danger/10 px-3 py-1.5 text-xs text-danger";

export const TABLE_HEAD = "px-2 py-1 text-left font-mono text-[10.5px] text-ink-faint";
export const TABLE_CELL = "px-2 py-1 align-top font-mono text-xs tabular-nums";

export function plotButton(on: boolean): string {
  return (
    `${INTERACTIVE} h-6 border border-transparent px-1.5 font-mono text-[10.5px] ` +
    (on
      ? "bg-plot-ink/16 text-plot-ink"
      : "text-plot-ink-dim hover:bg-plot-ink/10 hover:text-plot-ink")
  );
}

export function segment(selected: boolean): string {
  return (
    `${INTERACTIVE} h-6 px-2.5 text-xs font-medium ` +
    (selected ? "bg-accent/15 text-accent" : "text-ink-dim hover:bg-panel-2 hover:text-ink")
  );
}

export function segmentSm(selected: boolean): string {
  return (
    `${INTERACTIVE} h-5 px-1.5 font-mono text-[10.5px] ` +
    (selected ? "bg-accent/15 text-accent" : "text-ink-faint hover:bg-panel-2 hover:text-ink")
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

export function listItem(selected: boolean, highlighted: boolean): string {
  return (
    `${INSTANT} h-7 w-full justify-start px-2 text-xs ` +
    (selected ? "font-medium text-accent " : "text-ink-dim ") +
    (highlighted ? "bg-panel-2 " : "hover:bg-panel-2 ") +
    (selected ? "" : highlighted ? "text-ink" : "hover:text-ink")
  );
}

export type Options<T> = readonly { value: T; label: string; title?: string }[];

export const DIALOG_TITLE = "font-mono text-sm font-medium text-ink";

export const TAB_BAR = "flex items-stretch border-b border-line bg-panel";

export function tab(active: boolean): string {
  return (
    "-mb-px inline-flex h-7 items-center border-b-2 px-2.5 font-mono text-[11px] transition-colors " +
    "duration-100 disabled:pointer-events-none disabled:opacity-40 " +
    (active
      ? "border-accent text-ink"
      : "border-transparent text-ink-faint hover:border-line-strong hover:text-ink")
  );
}
