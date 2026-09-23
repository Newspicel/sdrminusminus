import type { LucideIcon } from "lucide-react";
import { Search } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import { Button, Input } from "./BaseControls";
import { FIELD, ICON_BTN_SM, LABEL } from "./controls";
import { Icon } from "./Icon";
import { Tip } from "./Tip";

export function Panel({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-3 p-3">{children}</div>;
}

export function PanelHint({ children }: { children: ReactNode }) {
  return <p className="text-xs text-ink-faint">{children}</p>;
}

export function PanelToolbar({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap items-center gap-2">{children}</div>;
}

export function SearchField({
  className,
  ...props
}: Omit<ComponentProps<typeof Input>, "className"> & { className?: string }) {
  return (
    <span className={`relative flex min-w-0 flex-1 ${className ?? ""}`}>
      <span className="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-ink-faint">
        <Icon glyph={Search} size={12} />
      </span>
      <Input className={`${FIELD} w-full pl-7`} {...props} />
    </span>
  );
}

export function List({
  title,
  aside,
  children,
}: {
  title?: string;
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1">
      {(title !== undefined || aside !== undefined) && (
        <header className="flex min-h-5 items-center justify-between gap-2 px-0.5">
          {title !== undefined && <span className={LABEL}>{title}</span>}
          {aside}
        </header>
      )}
      <ul className="flex flex-col divide-y divide-line overflow-hidden rounded-[3px] border border-line">
        {children}
      </ul>
    </section>
  );
}

export function ListRow({
  primary,
  secondary,
  lead,
  badge,
  hint,
  onSelect,
  disabled = false,
  actions,
  children,
}: {
  primary: ReactNode;
  secondary?: ReactNode;
  lead?: ReactNode;
  badge?: ReactNode;
  hint?: string;
  onSelect?: () => void;
  disabled?: boolean;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  const face = (
    <>
      <span className="flex min-w-0 items-center gap-2">
        {lead}
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-ink group-hover:text-accent">
          {primary}
        </span>
        {badge}
      </span>
      {secondary !== undefined && (
        <span className="legend block min-w-0 truncate tabular-nums">{secondary}</span>
      )}
    </>
  );
  return (
    <li className="flex flex-col gap-1.5 bg-panel px-2 py-1.5 transition-colors duration-100 hover:bg-panel-2/60">
      <div className="flex min-w-0 items-center gap-2">
        {onSelect === undefined ? (
          <div className="min-w-0 flex-1" title={hint}>
            {face}
          </div>
        ) : (
          <Button
            type="button"
            className="group min-w-0 flex-1 text-left disabled:pointer-events-none disabled:opacity-45 pointer-coarse:min-h-10"
            title={hint}
            disabled={disabled}
            onClick={onSelect}
          >
            {face}
          </Button>
        )}
        {actions !== undefined && (
          <span className="flex shrink-0 items-center gap-1">{actions}</span>
        )}
      </div>
      {children}
    </li>
  );
}

export function RowAction({
  label,
  glyph,
  onClick,
  disabled = false,
  danger = false,
}: {
  label: string;
  glyph: LucideIcon;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
}) {
  return (
    <Tip
      text={label}
      render={
        <Button
          type="button"
          aria-label={label}
          disabled={disabled}
          className={`${ICON_BTN_SM} ${danger ? "hover:text-danger" : ""}`}
          onClick={onClick}
        />
      }
    >
      <Icon glyph={glyph} size={12} />
    </Tip>
  );
}

export function RowLink({
  label,
  glyph,
  href,
}: {
  label: string;
  glyph: LucideIcon;
  href: string;
}) {
  return (
    <Tip
      text={label}
      render={<a aria-label={label} className={ICON_BTN_SM} href={href} download />}
    >
      <Icon glyph={glyph} size={12} />
    </Tip>
  );
}
