import type { ReactNode } from "react";
import { InfoTip } from "./InfoTip";

const GRID = "grid grid-cols-[fit-content(8rem)_minmax(0,1fr)] items-center gap-x-4 gap-y-2.5";

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
  title?: string;
  children: ReactNode;
}) {
  return (
    <div className={ROW}>
      <span className="legend flex items-center gap-1 wrap-anywhere">
        {label}
        {title !== undefined && <InfoTip text={title} />}
      </span>
      <span className="flex min-w-0 flex-wrap items-center gap-2">{children}</span>
    </div>
  );
}

export function SettingNote({ children }: { children: ReactNode }) {
  return <p className="col-span-2 text-xs text-ink-dim">{children}</p>;
}

export function GroupLine({
  label,
  hint,
  action,
}: {
  label: ReactNode;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <span className="legend col-span-2 flex min-h-5 items-center gap-2">
      <span aria-hidden className="w-2 shrink-0 border-t border-line" />
      <span className="flex items-center gap-1">
        {label}
        {hint !== undefined && <InfoTip text={hint} />}
      </span>
      <span aria-hidden className="min-w-0 flex-1 border-t border-line" />
      {action}
    </span>
  );
}

export function SettingGroup({
  label,
  hint,
  action,
  children,
}: {
  label: ReactNode;
  hint?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className={`${ROW} gap-y-2`}>
      <GroupLine label={label} hint={hint} action={action} />
      {children}
    </div>
  );
}
