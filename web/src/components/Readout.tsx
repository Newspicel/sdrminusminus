import type { ReactNode } from "react";
import { InfoTip } from "./InfoTip";
import { WithUnit } from "./Unit";

const GRID = "grid items-baseline gap-x-3 gap-y-1";

const COLUMNS = "grid-cols-[auto_minmax(0,1fr)]";

export function Readout({
  children,
  separated = true,
  className,
  columns = COLUMNS,
}: {
  children: ReactNode;
  separated?: boolean;
  className?: string;
  columns?: string;
}) {
  return (
    <div
      className={`flex flex-col gap-1 p-2 ${separated ? "border-t border-line" : ""} ${
        className ?? ""
      }`}
    >
      <div className={`${GRID} ${columns}`}>{children}</div>
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
      <span className="legend flex items-center gap-1 wrap-anywhere">
        {label}
        {title !== undefined && <InfoTip text={title} />}
      </span>
      <span className="min-w-0 font-mono text-xs tabular-nums text-ink">
        <WithUnit>{children}</WithUnit>
      </span>
    </>
  );
}
