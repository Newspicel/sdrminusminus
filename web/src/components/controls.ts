const INTERACTIVE =
  "inline-flex items-center gap-1.5 rounded-[3px] transition-colors duration-100 " +
  "disabled:opacity-45 disabled:pointer-events-none pointer-coarse:min-h-10";

/** Secondary: the default for a control that acts. Outline, not fill — see `BTN_PRIMARY`. */
export const BTN =
  `${INTERACTIVE} h-7 border border-line-strong bg-panel-2 px-2.5 text-xs text-ink ` +
  "hover:border-accent hover:text-accent";

export const BTN_PRIMARY =
  `${INTERACTIVE} h-7 border border-accent bg-accent px-3 text-xs font-medium text-bg ` +
  "hover:brightness-110";

/** Tertiary: rare or low-stakes, and toolbars over the plot where ink must stay cheap. */
export const BTN_QUIET =
  `${INTERACTIVE} h-7 border border-transparent px-2 text-xs text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink";

/** Irreversible. Its own hue, and never the pre-focused default. */
export const BTN_DANGER =
  `${INTERACTIVE} h-7 border border-line-strong bg-panel-2 px-2.5 text-xs text-ink ` +
  "hover:border-danger hover:bg-danger/10 hover:text-danger";

const ICON_BASE =
  `${INTERACTIVE} justify-center border border-transparent text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink";

/** Square-silhouette button for a single glyph; the padding, not the glyph, carries the size. */
export const ICON_BTN = `${ICON_BASE} size-7 pointer-coarse:size-10`;

/**
 * The same button where the row it sits in is shorter than a control: a face's title bar, a
 * settings group's header.
 *
 * Its own constant rather than `${ICON_BTN} size-5` at the call site — two `size-*` utilities set
 * the same properties, so the one Tailwind emits last wins whatever order they are written in,
 * and every such override was silently rendering at 28px.
 */
export const ICON_BTN_SM = `${ICON_BASE} size-5 pointer-coarse:size-10`;

/** Deliberately unsized: a filter strip wants its fields side by side and a settings row wants
 * them filling the column, so width is the caller's decision. */
export const FIELD =
  `${INTERACTIVE} h-7 min-w-0 border border-line-strong bg-panel-2 px-2 font-mono text-xs ` +
  "text-ink placeholder:text-ink-faint hover:border-accent-dim";

export const LABEL =
  "inline-flex items-center gap-2 whitespace-nowrap font-mono text-[10px] tracking-[0.09em] " +
  "uppercase text-ink-faint";

/** Read-only status pill: a value plus its legend, on a recessed well. */
export const CHIP =
  "inline-flex h-7 items-center gap-1.5 rounded-[3px] border border-line bg-panel-2 px-2 " +
  "font-mono text-xs text-ink";

export const SURFACE = "rounded-md border border-line-strong bg-panel-3 shadow-pop";

/** A refusal or a fault, stated in place. Never the only signal — the control that caused it
 * keeps its own state. */
export const ALERT =
  "rounded-[3px] border border-danger bg-danger/10 px-3 py-1.5 font-mono text-xs text-danger";

/** Column heading and body cell for the app's data tables — the decoder log and the target
 * lists. Figures are mono and tabular so a column can be compared down its length. */
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

/** Segmented / toggle control member. `selected` is carried by fill *and* text weight, never by
 * colour alone. */
export function segment(selected: boolean): string {
  return (
    `${INTERACTIVE} h-7 px-2.5 text-xs ` +
    (selected
      ? "bg-accent/15 font-medium text-accent"
      : "text-ink-dim hover:bg-panel-2 hover:text-ink")
  );
}

/** [`segment`] for a face's title bar, which is shorter than a control row. Its own constant
 * rather than an override at the call site, for the reason `ICON_BTN_SM` is: two `h-*` utilities
 * set the same property, and the one Tailwind emits last wins whatever order they are written. */
export function segmentSm(selected: boolean): string {
  return (
    `${INTERACTIVE} h-5 px-1.5 font-mono text-[10px] tracking-[0.09em] uppercase ` +
    (selected
      ? "bg-accent/15 font-medium text-accent"
      : "text-ink-faint hover:bg-panel-2 hover:text-ink")
  );
}

/** A choice list for `Select` and `Segmented`. Typed off the value union at the call site, so a
 * renamed or added wire variant breaks here instead of shipping an option the server rejects. */
export type Options<T> = readonly { value: T; label: string }[];
