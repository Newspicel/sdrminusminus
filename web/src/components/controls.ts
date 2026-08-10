// The control kit (DESIGN.md §4, §9). One place so every strip, panel and popover in the UI is
// built from the same parts — a control that needs a look not in here is a missing entry, not a
// local class string.
//
// `pointer-coarse:min-h-10` on every interactive class is the target floor: compact density
// buys space by trimming padding, never hit area.

const INTERACTIVE =
  "inline-flex items-center gap-1.5 rounded-[3px] transition-colors duration-100 " +
  "disabled:opacity-45 disabled:pointer-events-none pointer-coarse:min-h-10";

/** Secondary: the default for a control that acts. Outline, not fill — see `BTN_PRIMARY`. */
export const BTN =
  `${INTERACTIVE} h-7 border border-line-strong bg-panel-2 px-2.5 text-xs text-ink ` +
  "hover:border-accent hover:text-accent";

/** The single most likely next action in a view. Exactly one per view (DESIGN.md §9). */
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

/** Square-silhouette button for a single glyph; the padding, not the glyph, carries the size. */
export const ICON_BTN =
  `${INTERACTIVE} size-7 justify-center border border-transparent text-ink-dim ` +
  "hover:bg-panel-2 hover:text-ink pointer-coarse:size-10";

/** Deliberately unsized: a filter strip wants its fields side by side and a settings row wants
 * them filling the column, so width is the caller's decision. */
export const FIELD =
  `${INTERACTIVE} h-7 min-w-0 border border-line-strong bg-panel-2 px-2 font-mono text-xs ` +
  "text-ink placeholder:text-ink-faint hover:border-accent-dim";

/** A label beside its control, in the silkscreen voice (DESIGN.md §3) — the typography is on
 * the label itself so a bare text child is styled without a wrapper. The 8px gap against the
 * 20px between groups keeps the 2x proximity ratio that makes the pairing readable without a
 * box. */
export const LABEL =
  "inline-flex items-center gap-2 whitespace-nowrap font-mono text-[10px] tracking-[0.09em] " +
  "uppercase text-ink-faint";

/** Read-only status pill: a value plus its legend, on a recessed well. */
export const CHIP =
  "inline-flex h-7 items-center gap-1.5 rounded-[3px] border border-line bg-panel-2 px-2 " +
  "font-mono text-xs text-ink";

/** Elevation 3. The only place in the UI that casts a shadow (DESIGN.md §4). */
export const SURFACE = "rounded-md border border-line-strong bg-panel-3 shadow-pop";

/** A control drawn *on* the plot, where the chrome palette does not reach: inside that
 * rectangle the colormap owns hue, so the toolbar separates by luminance alone (DESIGN.md §2).
 * The legend typography is inlined rather than nested, so the hover state can carry the colour. */
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

/** A choice list for `Select` and `Segmented`. Typed off the value union at the call site, so a
 * renamed or added wire variant breaks here instead of shipping an option the server rejects. */
export type Options<T> = readonly { value: T; label: string }[];
