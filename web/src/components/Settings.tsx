import type { ReactNode } from "react";

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

export function SettingNote({ children }: { children: ReactNode }) {
  return <p className="col-span-2 text-xs text-ink-dim">{children}</p>;
}

export function SettingGroup({
  label,
  action,
  children,
}: {
  label: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className={`${ROW} gap-y-2 border-t border-line pt-2 first:border-t-0 first:pt-0`}>
      <span className="legend col-span-2 flex min-h-5 items-center justify-between gap-2">
        {label}
        {action}
      </span>
      {children}
    </div>
  );
}
