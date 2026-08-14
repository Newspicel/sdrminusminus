import { buttonVariants } from "@/components/ui/button";

export const LABEL =
  "inline-flex items-center gap-2 whitespace-nowrap font-mono text-[10px] tracking-[0.09em] " +
  "uppercase text-muted-foreground/70";

/** `LABEL` for a row whose words toggle a box, so the pointer says the text is part of the
 * control. Not for a label over a field — clicking that only moves focus. */
export const CHECK_LABEL = `${LABEL} cursor-pointer`;

export function plotButton(on: boolean): string {
  return (
    `${buttonVariants({ variant: "ghost", size: "xs" })} h-6 px-1.5 font-mono text-[10px] tracking-[0.09em] uppercase ` +
    (on
      ? "bg-plot-ink/15 text-plot-ink"
      : "text-plot-ink-dim hover:bg-plot-ink/12 hover:text-plot-ink")
  );
}

/** A choice list for `Select` and `Segmented`. Typed off the value union at the call site, so a
 * renamed or added wire variant breaks here instead of shipping an option the server rejects. */
export type Options<T> = readonly { value: T; label: string }[];
