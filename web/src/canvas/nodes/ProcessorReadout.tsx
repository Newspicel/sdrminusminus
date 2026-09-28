import type { ReactNode } from "react";
import { CHIP_SM } from "../../components/controls";
import type { ProcessorStatus } from "../../lib/types";
import { processorFaults } from "./processorFace";

export interface Chip {
  label: string;
  title: string;
  danger?: boolean;
}

export function ProcessorReadout({
  children,
  columns = 2,
}: {
  children: ReactNode;
  columns?: 2 | 3 | 4;
}) {
  const grid = columns === 4 ? "grid-cols-4" : columns === 3 ? "grid-cols-3" : "grid-cols-2";
  return <dl className={`grid shrink-0 ${grid} gap-x-3 gap-y-0.5`}>{children}</dl>;
}

export function ReadoutCell({
  label,
  value,
  title,
  danger = false,
  wide = false,
}: {
  label: string;
  value: string;
  title?: string;
  danger?: boolean;
  wide?: boolean;
}) {
  return (
    <div
      className={`flex min-w-0 items-baseline justify-between gap-1 ${wide ? "col-span-full" : ""}`}
      title={title}
    >
      <dt className="legend">{label}</dt>
      <dd
        className={`truncate font-mono text-xs tabular-nums ${danger ? "text-danger" : "text-ink"}`}
      >
        {value}
      </dd>
    </div>
  );
}

export function ProcessorChips({ chips }: { chips: readonly Chip[] }) {
  if (chips.length === 0) {
    return null;
  }
  return (
    <div className="flex flex-wrap gap-1 px-2 pb-2">
      {chips.map((chip) => (
        <span
          key={chip.label}
          title={chip.title}
          className={`${CHIP_SM} ${chip.danger === true ? "border-danger/60 text-danger" : "text-warn"}`}
        >
          {chip.label}
        </span>
      ))}
    </div>
  );
}

export function faultChips(status: ProcessorStatus | null): Chip[] {
  return processorFaults(status).map((row) => ({
    label: `${row.label} ${row.count}`,
    title: row.title,
    danger: true,
  }));
}

export function ProcessorFaults({ status }: { status: ProcessorStatus | null }) {
  const error = status?.error ?? null;
  return (
    <>
      {error !== null && (
        <p role="alert" title={error} className="truncate px-2 pb-1 text-xs text-danger">
          {error}
        </p>
      )}
      <ProcessorChips chips={faultChips(status)} />
    </>
  );
}
