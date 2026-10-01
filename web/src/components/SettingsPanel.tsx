import type { ReactNode } from "react";
import { InfoTip } from "./InfoTip";

export function SettingsPanel({ children }: { children: ReactNode }) {
  return <div className="flex flex-col divide-y divide-line">{children}</div>;
}

export function SettingsSection({
  name,
  hint,
  aside,
  children,
}: {
  name: string;
  hint?: string;
  aside?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2 px-3 py-2.5">
      <header className="flex min-h-5 items-center justify-between gap-3">
        <span className="legend flex items-center gap-1 text-ink-dim">
          {name}
          {hint !== undefined && <InfoTip text={hint} />}
        </span>
        {aside}
      </header>
      {children}
    </section>
  );
}
