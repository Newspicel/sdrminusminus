import { useLayoutEffect, useRef, useState } from "react";
import { Button } from "../../components/BaseControls";
import type { BandIdentity } from "../../components/bandPlan";
import {
  identify,
  serviceEdge,
  serviceFill,
  serviceLabel,
  spansIn,
  suggestedAt,
} from "../../components/bandPlan";
import { CHIP, LABEL, SURFACE } from "../../components/controls";
import { formatHz } from "../../components/format";
import { type SpectrumView, spanToOffset, viewWidth } from "../../components/spectrumView";
import type { ChannelParams } from "../../lib/types";
import { useBandPlan } from "../../lib/useBandPlan";

const ROW_H = 16;
const LABEL_MIN = 0.07;

export function BandRuler({
  centerHz,
  spanHz,
  view,
  onTune,
}: {
  centerHz: number;
  spanHz: number;
  view: SpectrumView;
  onTune: (hz: number, suggested: ChannelParams | null) => void;
}) {
  const { plan, ruler } = useBandPlan();
  const [pick, setPick] = useState<{ hz: number; at: number; frame: string } | null>(null);
  const rulerRef = useRef<HTMLDivElement>(null);

  const frame = `${centerHz}:${spanHz}:${view.start}:${view.end}`;
  const picked = pick?.frame === frame ? pick : null;

  if (!ruler || plan === null || !(spanHz > 0)) {
    return null;
  }

  const visibleHz = spanHz * viewWidth(view);
  const lowHz = centerHz + spanToOffset(view.start, spanHz);
  const hzAt = (fraction: number): number => lowHz + fraction * visibleHz;

  const onPick = (event: React.MouseEvent<HTMLElement>): void => {
    const rect = rulerRef.current?.getBoundingClientRect();
    if (rect === undefined || rect.width === 0) {
      return;
    }
    const at = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    setPick({ hz: hzAt(at), at, frame });
  };

  return (
    <div ref={rulerRef} data-plot-chrome className="relative shrink-0 bg-bg">
      {plan.lanes.map((lane) => {
        const spans = spansIn(plan, lane, lowHz, visibleHz);
        return (
          <Button
            key={lane.id}
            type="button"
            className="relative block w-full cursor-help border-b border-line/60 last:border-b-0"
            style={{ height: `${ROW_H}px` }}
            onClick={(event) => {
              event.stopPropagation();
              onPick(event);
            }}
            onPointerDown={(event) => event.stopPropagation()}
            aria-label={`${lane.name} — click to identify a frequency`}
          >
            {spans.map((span) => (
              <span
                key={`${span.allocation.id}:${span.block.start_hz}`}
                aria-hidden
                className={`absolute inset-y-0 overflow-hidden ${serviceFill(span.allocation.service)}`}
                style={{ left: `${span.left * 100}%`, width: `${span.width * 100}%` }}
              >
                {span.startsInside && (
                  <span
                    className={`absolute inset-y-0 left-0 w-px ${serviceEdge(span.allocation.service)}`}
                  />
                )}
                {span.width >= LABEL_MIN && (
                  <span className="absolute inset-y-0 left-1 flex items-center whitespace-nowrap font-mono text-[10px] text-ink">
                    {span.allocation.name}
                  </span>
                )}
              </span>
            ))}
          </Button>
        );
      })}

      {picked !== null && (
        <IdentifyCard
          hz={picked.hz}
          at={picked.at}
          found={identify(plan, picked.hz)}
          layerName={(id) => plan.layers.find((layer) => layer.id === id)?.authority ?? id}
          onTune={onTune}
          onClose={() => setPick(null)}
        />
      )}
    </div>
  );
}

function IdentifyCard({
  hz,
  at,
  found,
  layerName,
  onTune,
  onClose,
}: {
  hz: number;
  at: number;
  found: readonly BandIdentity[];
  layerName: (id: string) => string;
  onTune: (hz: number, suggested: ChannelParams | null) => void;
  onClose: () => void;
}) {
  const suggested = suggestedAt(found);
  const cardRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const returnTo = document.activeElement;
    cardRef.current?.focus();
    return () => {
      if (returnTo instanceof HTMLElement) {
        returnTo.focus();
      }
    };
  }, [hz]);
  return (
    <div
      ref={cardRef}
      tabIndex={-1}
      className={`absolute top-full z-20 mt-1 flex w-64 -translate-x-1/2 flex-col gap-1.5 p-2 outline-none ${SURFACE}`}
      style={{ left: `clamp(8rem, ${at * 100}%, calc(100% - 8rem))` }}
      onKeyDown={(event) => event.key === "Escape" && onClose()}
      role="dialog"
      aria-label={`Allocation at ${formatHz(hz)}`}
    >
      <div className="flex items-baseline justify-between gap-2">
        <span className="font-mono text-sm text-ink tabular-nums">{formatHz(hz)}</span>
        <Button
          type="button"
          className="text-ink-faint hover:text-ink"
          aria-label="Close"
          onClick={onClose}
        >
          ✕
        </Button>
      </div>

      {found.length === 0 && (
        <span className="text-xs text-ink-dim">
          Nothing allocated here in this region's tables.
        </span>
      )}

      {found.map((entry) => (
        <BandDetail
          key={entry.laneId}
          entry={entry}
          layerName={layerName}
          covered={entry.laneId === found[0]?.laneId}
        />
      ))}

      {found.length > 0 && (
        <Button
          type="button"
          className="mt-0.5 h-7 rounded-[3px] border border-accent bg-accent/12 px-2 font-mono text-xs text-accent hover:bg-accent/20"
          onClick={() => {
            onTune(hz, suggested);
            onClose();
          }}
        >
          Tune {formatHz(hz)}
          {suggested !== null && ` · ${suggested.type.toUpperCase()}`}
        </Button>
      )}
    </div>
  );
}

function BandDetail({
  entry,
  layerName,
  covered,
}: {
  entry: BandIdentity;
  layerName: (id: string) => string;
  covered: boolean;
}) {
  const { allocation } = entry;
  return (
    <div className="flex flex-col gap-0.5 border-t border-line pt-1.5 first:border-t-0 first:pt-0">
      <div className="flex items-center gap-1.5">
        <span
          aria-hidden
          className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
        />
        <span className="min-w-0 truncate text-sm text-ink">{allocation.name}</span>
        {!allocation.primary && (
          <span className={CHIP} title="Must accept interference from every primary service">
            secondary
          </span>
        )}
      </div>
      {allocation.official_name !== allocation.name && (
        <span className="truncate font-mono text-[11px] text-ink-dim">
          {allocation.official_name}
        </span>
      )}
      <span className={LABEL}>
        {serviceLabel(allocation.service)} · {layerName(allocation.layer)} ·{" "}
        {formatHz(allocation.start_hz)}–{formatHz(allocation.stop_hz)}
        {allocation.reference != null && ` · ${allocation.reference}`}
      </span>
      {allocation.channel_step_hz != null && (
        <span className={CHIP}>{formatHz(allocation.channel_step_hz)} channels</span>
      )}
      {allocation.notes != null && (
        <p className="text-xs leading-snug text-ink-dim">{allocation.notes}</p>
      )}
      {covered &&
        entry.covered.map((under) => (
          <span key={under.id} className="text-[11px] text-ink-faint">
            {under.layer === allocation.layer ? "also" : `over ${layerName(under.layer)}:`}{" "}
            {under.name}
          </span>
        ))}
    </div>
  );
}
