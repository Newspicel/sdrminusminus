// Panel wrapper for the strip under the spectrum. Always open on desktop; on phones the
// header becomes a ≥40px toggle so the spectrum keeps the screen (PLAN §10: phone-usable).
import { type ReactNode, useState } from "react";

const HEADER =
  "border-b border-line bg-panel px-4 text-xs font-semibold uppercase tracking-wider text-ink-dim";

export function PanelSection({
  title,
  defaultOpen = true,
  children,
}: {
  title: string;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className="flex min-h-0 flex-col">
      <button
        type="button"
        className={`flex min-h-10 items-center justify-between text-left md:hidden ${HEADER}`}
        onClick={() => setOpen(!open)}
        aria-expanded={open}
      >
        {title}
        <span className="font-mono">{open ? "−" : "+"}</span>
      </button>
      <div className={`hidden min-h-10 items-center md:flex ${HEADER}`}>{title}</div>
      <div className={`${open ? "block" : "hidden"} min-h-0 md:block`}>{children}</div>
    </section>
  );
}
