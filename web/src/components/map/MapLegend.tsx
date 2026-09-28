import type { ReactNode } from "react";
import type { BasemapKind } from "../../lib/map/basemap";
import { type OverlayCounts, TRACK_COLOR } from "../../lib/map/df";
import { HEAT_OPACITY } from "../../lib/map/heat";
import { KIND_STYLE, type MapKind } from "../../lib/map/layers";
import { SIGNAL_GRADIENT } from "../../lib/map/signal";
import { SIGNAL_MAX_DBFS, SIGNAL_MIN_DBFS } from "../../lib/signalSurvey";
import { colormapGradient } from "../plotFrame";
import type { Counts } from "./mapState";

const ROW = "flex items-center gap-2 font-mono text-[10px] tabular-nums";
const BADGE = "rounded border border-line bg-bg/85 px-2 py-1 font-mono text-[10px] text-ink-dim";

function Row({
  swatch,
  label,
  value,
  title,
  danger = false,
}: {
  swatch: ReactNode;
  label: string;
  value?: string | number;
  title?: string;
  danger?: boolean;
}) {
  return (
    <div className={title === undefined ? ROW : `${ROW} pointer-events-auto`} title={title}>
      {swatch}
      <span className={danger ? "text-danger" : "text-ink-dim"}>{label}</span>
      {value !== undefined && (
        <span className={`ml-auto ${danger ? "text-danger" : "text-ink"}`}>{value}</span>
      )}
    </div>
  );
}

function Dot({ color, className }: { color?: string; className?: string }) {
  return (
    <span
      className={`inline-block h-2 w-2 shrink-0 rounded-full ${className ?? ""}`}
      style={color === undefined ? undefined : { backgroundColor: color }}
    />
  );
}

export function MapLegend({
  kinds,
  counts,
  positionCount,
  signalCells,
  overlay,
  heat,
  headings,
  basemap,
}: {
  kinds: readonly MapKind[];
  counts: Counts;
  positionCount: number | null;
  signalCells: number | null;
  overlay: OverlayCounts;
  heat: boolean;
  headings: boolean;
  basemap: BasemapKind;
}) {
  const hasRows =
    kinds.length > 0 ||
    positionCount !== null ||
    signalCells !== null ||
    heat ||
    overlay.bearings + overlay.echoes + overlay.tracks + overlay.unplaced.length > 0;
  return (
    <div className="pointer-events-none absolute top-2 left-2 flex flex-col items-start gap-1">
      {hasRows && (
        <div className="flex flex-col gap-1 rounded border border-line bg-bg/85 px-2 py-1.5">
          {kinds.map((kind) => (
            <Row
              key={kind}
              swatch={<Dot color={KIND_STYLE[kind].color} />}
              label={KIND_STYLE[kind].title}
              value={counts[kind]}
            />
          ))}
          {positionCount !== null && (
            <Row swatch={<Dot className="bg-accent" />} label="GPS trail" value={positionCount} />
          )}
          {signalCells !== null && <SignalRows cells={signalCells} />}
          <OverlayRows overlay={overlay} heat={heat} />
        </div>
      )}
      {basemap === "blank" && <div className={BADGE}>no basemap</div>}
      {!headings && <div className={BADGE}>no headings</div>}
    </div>
  );
}

function SignalRows({ cells }: { cells: number }) {
  return (
    <div className="flex min-w-36 flex-col gap-1 font-mono text-[10px] tabular-nums">
      <div className="flex items-center justify-between gap-3">
        <span className="text-ink-dim">Signal cells</span>
        <span className="text-ink">{cells}</span>
      </div>
      <div className="h-1.5 w-full rounded-full" style={{ background: SIGNAL_GRADIENT }} />
      <div className="flex justify-between text-ink-faint">
        <span>{SIGNAL_MIN_DBFS} dBFS</span>
        <span>{SIGNAL_MAX_DBFS} dBFS</span>
      </div>
    </div>
  );
}

function OverlayRows({ overlay, heat }: { overlay: OverlayCounts; heat: boolean }) {
  return (
    <>
      {overlay.bearings > 0 && (
        <Row swatch={<Dot className="bg-accent" />} label="Bearings" value={overlay.bearings} />
      )}
      {overlay.echoes > 0 && (
        <Row swatch={<Dot color={TRACK_COLOR} />} label="Echoes" value={overlay.echoes} />
      )}
      {overlay.tracks > 0 && (
        <Row swatch={<Dot color={TRACK_COLOR} />} label="Tracks" value={overlay.tracks} />
      )}
      {heat && (
        <Row
          swatch={
            <span
              className="inline-block h-2 w-4 shrink-0 rounded-[2px]"
              style={{ background: colormapGradient("inferno", "to right"), opacity: HEAT_OPACITY }}
            />
          }
          label="Heat"
        />
      )}
      {overlay.unplaced.length > 0 && (
        <Row
          swatch={<Dot className="bg-danger" />}
          label="No position"
          value={overlay.unplaced.length}
          title={overlay.unplaced.join(", ")}
          danger
        />
      )}
    </>
  );
}
