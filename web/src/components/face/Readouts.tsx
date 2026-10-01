import type { ReactNode } from "react";
import { InfoTip } from "../InfoTip";
import { GroupLine } from "../Settings";
import { WithUnit } from "../Unit";
import type { ChipTone } from "./Chips";

const COLUMNS = {
  1: "grid-cols-1",
  2: "grid-cols-2",
  3: "grid-cols-3",
  4: "grid-cols-4",
  fit: "grid-cols-[repeat(auto-fill,minmax(8.5rem,1fr))]",
} as const;

const TONE: Record<ChipTone, string> = {
  ok: "text-ok",
  warn: "text-warn",
  danger: "text-danger",
};

export function Readouts({
  children,
  columns = 1,
  label,
  ruled = true,
  padded = true,
  className,
}: {
  children: ReactNode;
  columns?: keyof typeof COLUMNS;
  label?: ReactNode;
  ruled?: boolean;
  padded?: boolean;
  className?: string;
}) {
  return (
    <div
      className={[
        "flex shrink-0 flex-col gap-1",
        padded && "p-2",
        ruled && "border-t border-line first:border-t-0",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {label !== undefined && <GroupLine label={label} />}
      <dl className={`grid ${COLUMNS[columns]} gap-x-3 gap-y-0.5`}>{children}</dl>
    </div>
  );
}

export function Readout({
  label,
  title,
  hint,
  tone,
  wide = false,
  children,
}: {
  label: ReactNode;
  title?: string;
  hint?: string;
  tone?: ChipTone;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <div
      className={`flex min-w-0 items-baseline justify-between gap-2 ${wide ? "col-span-full" : ""}`}
      title={title}
    >
      <dt className="legend flex shrink-0 items-center gap-1">
        {label}
        {hint !== undefined && <InfoTip text={hint} />}
      </dt>
      <dd
        className={`min-w-0 truncate text-right font-mono text-xs tabular-nums ${tone === undefined ? "text-ink" : TONE[tone]}`}
      >
        <WithUnit>{children}</WithUnit>
      </dd>
    </div>
  );
}
