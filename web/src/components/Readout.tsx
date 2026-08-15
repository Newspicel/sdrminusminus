import type { ReactNode } from "react";

const GRID = "grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-1";

/** A block of values the operator reads rather than sets. */
export function Readout({
  children,
  separated = true,
  className,
}: {
  children: ReactNode;
  /** `false` where the block already sits directly under a header. */
  separated?: boolean;
  className?: string;
}) {
  return (
    <div
      className={`flex flex-col gap-1 p-2 ${separated ? "border-t border-line" : ""} ${
        className ?? ""
      }`}
    >
      <div className={GRID}>{children}</div>
    </div>
  );
}

export function ReadoutRow({
  label,
  title,
  children,
}: {
  label: ReactNode;
  title?: string;
  children: ReactNode;
}) {
  return (
    <>
      <span className="legend wrap-anywhere" title={title}>
        {label}
      </span>
      <span className="min-w-0 font-mono text-xs tabular-nums text-ink">{children}</span>
    </>
  );
}
