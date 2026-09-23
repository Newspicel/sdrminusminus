import type { CSSProperties, ReactNode } from "react";
import { Tip } from "./Tip";
import { splitUnit, unitTip } from "./units";

export function Unit({ symbol, className }: { symbol: string; className?: string }) {
  const tip = unitTip(symbol);
  if (tip === undefined) {
    return <span className={className}>{symbol}</span>;
  }
  return (
    <Tip text={tip} render={<span className={`cursor-help ${className ?? ""}`} />}>
      {symbol}
    </Tip>
  );
}

export function WithUnit({ children }: { children: ReactNode }) {
  const parts = typeof children === "string" ? splitUnit(children) : undefined;
  if (parts === undefined) {
    return children;
  }
  const [value, symbol] = parts;
  return (
    <>
      {value} <Unit symbol={symbol} />
    </>
  );
}

export function unitPadding(symbol: string): CSSProperties {
  return { paddingRight: `calc(${symbol.length}ch + 0.75rem)` };
}

export function InFieldUnit({ symbol }: { symbol: string }) {
  return (
    <Unit
      symbol={symbol}
      className="absolute top-1/2 right-2 -translate-y-1/2 font-mono text-xs text-ink-faint"
    />
  );
}

export function FieldUnitFrame({
  symbol,
  className,
  children,
}: {
  symbol: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span className={`relative inline-block ${className ?? ""}`}>
      {children}
      <InFieldUnit symbol={symbol} />
    </span>
  );
}
