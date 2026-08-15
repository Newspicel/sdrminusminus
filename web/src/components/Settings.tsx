import type { ReactNode } from "react";

// One grid for the whole block with every row a `subgrid` of it, so the label track is measured
// once across all rows instead of each label sizing to its own words. No breakpoints: the faces
// these render in are a fixed size (`NODE_SIZE`), so the track is the widest label present and
// the control gets the rest — `fit-content` caps the longest name (`unshift on space`) rather
// than letting it take the row.
const GRID = "grid grid-cols-[fit-content(7.5rem)_minmax(0,1fr)] items-center gap-x-3 gap-y-2";

const ROW = "col-span-2 grid grid-cols-subgrid items-center";

export function Settings({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={`${GRID} ${className ?? ""}`}>{children}</div>;
}

export function SettingRow({
  label,
  title,
  children,
}: {
  label: ReactNode;
  /** The driver's own key, where the label is a rendering of it. */
  title?: string;
  children: ReactNode;
}) {
  return (
    <div className={ROW}>
      <span className="legend wrap-anywhere" title={title}>
        {label}
      </span>
      <span className="flex min-w-0 items-center gap-2">{children}</span>
    </div>
  );
}

/** A line about the block rather than about one setting: it spans both tracks, so it is not read
 * as the value of the row above it. */
export function SettingNote({ children }: { children: ReactNode }) {
  return <p className="col-span-2 text-xs text-ink-dim">{children}</p>;
}

/** A named run of rows — one radio lane, one scan range. Also a subgrid, so its rows keep the
 * track the block measured. */
export function SettingGroup({
  label,
  action,
  children,
}: {
  label: ReactNode;
  /** What can be done to the group as a whole, level with its name — removing one of a repeated
   * set. Not a setting, so it does not take a row of its own. */
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    // The rule separates this group from what is above it, so a group that opens the block has
    // nothing to be separated from — the face's own header is already that line.
    <div className={`${ROW} gap-y-2 border-t border-line pt-2 first:border-t-0 first:pt-0`}>
      {/* `min-h-5` is the action button's own height, held whether or not there is one: a remove
          that appears with the second group would otherwise grow every group's header row, and
          adding one range would shift the rows of the range above it. */}
      <span className="legend col-span-2 flex min-h-5 items-center justify-between gap-2">
        {label}
        {action}
      </span>
      {children}
    </div>
  );
}
