import type { ReactNode } from "react";
import type { ChipTone } from "./Chips";

const TONE: Record<ChipTone, string> = {
  ok: "text-ok",
  warn: "text-warn",
  danger: "text-danger",
};

export function FaceStats({ children }: { children: ReactNode }) {
  return <span className="mr-auto flex min-w-0 flex-wrap items-center gap-3">{children}</span>;
}

export function Stat({
  label,
  title,
  tone,
  children,
}: {
  label: ReactNode;
  title: string;
  tone?: ChipTone;
  children?: ReactNode;
}) {
  return (
    <span
      className={`inline-flex items-center gap-1 font-mono text-[11px] whitespace-nowrap ${
        children === undefined && tone !== undefined ? TONE[tone] : "text-ink-faint"
      }`}
      title={title}
    >
      {label}
      {children !== undefined && (
        <b className={`font-medium ${tone === undefined ? "text-ink" : TONE[tone]}`}>{children}</b>
      )}
    </span>
  );
}
