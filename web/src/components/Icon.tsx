import type { LucideIcon } from "lucide-react";

export function Icon({
  glyph: Glyph,
  size = 14,
  filled = false,
}: {
  glyph: LucideIcon;
  size?: 12 | 14 | 16;
  filled?: boolean;
}) {
  return (
    <Glyph
      aria-hidden
      size={size}
      strokeWidth={1.75}
      className={filled ? "shrink-0 fill-current" : "shrink-0"}
    />
  );
}
