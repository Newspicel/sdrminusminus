import { X } from "lucide-react";
import { KIND_STYLE, type TargetDetail } from "../../lib/map/layers";
import { Button } from "../BaseControls";
import { formatMhz } from "../format";
import { Icon } from "../Icon";

function formatUtc(ms: number): string {
  return `${new Date(ms).toISOString().slice(11, 19)}Z`;
}

export function TargetCard({ detail, onClose }: { detail: TargetDetail; onClose: () => void }) {
  return (
    <div className="absolute inset-x-2 bottom-2 rounded border border-line bg-panel/95 md:inset-x-auto md:right-2 md:w-64">
      <div className="flex items-center justify-between gap-2 border-b border-line px-2 py-1">
        <span
          className="truncate font-mono text-sm"
          style={{ color: KIND_STYLE[detail.kind].color }}
        >
          {detail.label}
        </span>
        <Button
          type="button"
          className="shrink-0 px-1 font-mono text-xs text-ink-dim hover:text-ink"
          onClick={onClose}
          aria-label="Clear target selection"
        >
          <Icon glyph={X} size={12} />
        </Button>
      </div>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 px-2 py-1.5">
        {detail.rows.map(([label, value]) => (
          <div key={label} className="col-span-2 grid grid-cols-subgrid">
            <dt className="text-[11px] text-ink-faint">{label}</dt>
            <dd className="truncate text-right font-mono text-xs tabular-nums text-ink">{value}</dd>
          </div>
        ))}
      </dl>
      <div className="border-t border-line px-2 py-1 font-mono text-[10px] tabular-nums text-ink-dim">
        {formatMhz(detail.freqHz)} · last seen {formatUtc(detail.lastSeen)}
      </div>
    </div>
  );
}
