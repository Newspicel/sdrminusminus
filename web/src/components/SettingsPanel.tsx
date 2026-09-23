import type { ReactNode } from "react";
import { Button } from "./BaseControls";
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

export function ToggleChip({
  label,
  on,
  onClick,
  children,
}: {
  label: string;
  on: boolean;
  onClick: () => void;
  children?: ReactNode;
}) {
  return (
    <Button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={`flex h-7 items-center gap-2 rounded-[3px] border px-2 font-mono text-[11px] transition-colors duration-100 ${
        on
          ? "border-accent-dim bg-accent/12 text-accent"
          : "border-line bg-well text-ink-dim hover:border-line-strong hover:text-ink"
      }`}
    >
      {children}
      {label}
    </Button>
  );
}
