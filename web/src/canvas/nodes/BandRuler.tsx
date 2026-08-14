import { useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import type { BandIdentity } from "../../components/bandPlan";
import {
  identify,
  serviceEdge,
  serviceFill,
  serviceLabel,
  spansIn,
  suggestedAt,
} from "../../components/bandPlan";
import { LABEL } from "../../components/controls";
import { formatHz } from "../../components/format";
import { type SpectrumView, spanToOffset, viewWidth } from "../../components/spectrumView";
import type { ChannelParams } from "../../lib/types";
import { useBandPlan } from "../../lib/useBandPlan";

/** Row height. Two lanes plus their rules cost 34px of a scope face — enough for a 10px name
 * and no more, because everything it takes comes off the waterfall. */
const ROW_H = 16;
/** Below this fraction of the window a block has no room for its name; the popover is how it is
 * read. Not a measurement — a fraction is all the geometry knows — but the ruler is at least
 * 200px wide in any face that fits a scope, so 7% is ~14px. */
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
  /** Tune to this frequency, applying the band's suggested mode when it has one. */
  onTune: (hz: number, suggested: ChannelParams | null) => void;
}) {
  const { plan, ruler } = useBandPlan();
  const [picked, setPicked] = useState<{ hz: number; at: number } | null>(null);
  const rulerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  // A pan or a zoom moves the spectrum out from under an open popover, and a card still naming
  // the old frequency is worse than no card. The view is the trigger, not the pointer, so this
  // also closes it when the *radio* is retuned by someone else.
  useEffect(() => setPicked(null), [view, centerHz, spanHz]);

  if (!ruler || plan === null || !(spanHz > 0)) {
    return null;
  }

  const visibleHz = spanHz * viewWidth(view);
  const lowHz = centerHz + spanToOffset(view.start, spanHz);
  const hzAt = (fraction: number): number => lowHz + fraction * visibleHz;

  const onPick = (event: React.MouseEvent<HTMLButtonElement>): void => {
    const rect = rulerRef.current?.getBoundingClientRect();
    if (rect === undefined || rect.width === 0) {
      return;
    }
    const at = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    triggerRef.current = event.currentTarget;
    setPicked({ hz: hzAt(at), at });
  };
  const closePicked = (): void => {
    setPicked(null);
    requestAnimationFrame(() => triggerRef.current?.focus());
  };

  return (
    <div ref={rulerRef} data-plot-chrome className="relative shrink-0 bg-background">
      {plan.lanes.map((lane) => {
        const spans = spansIn(plan, lane, lowHz, visibleHz);
        return (
          <Button
            key={lane.id}
            type="button"
            className="relative block w-full cursor-help border-b border-border/60 last:border-b-0"
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
                  <span className="absolute inset-y-0 left-1 flex items-center whitespace-nowrap font-mono text-[10px] text-foreground">
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
          onClose={closePicked}
        />
      )}
    </div>
  );
}

/** What is at the clicked frequency, one section per lane, most authoritative first. Anchored to
 * the click but kept inside the face: a card that hangs off the edge of a node is unreadable and
 * a node is not a viewport. */
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
  useEffect(() => cardRef.current?.focus(), []);
  return (
    <Card
      ref={cardRef}
      tabIndex={-1}
      size="sm"
      className="absolute top-full z-20 mt-1 w-64 -translate-x-1/2 gap-1.5 p-2"
      style={{ left: `clamp(8rem, ${at * 100}%, calc(100% - 8rem))` }}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
      role="dialog"
      aria-label={`Allocation at ${formatHz(hz)}`}
    >
      <div className="flex items-baseline justify-between gap-2">
        <span className="font-mono text-sm text-foreground tabular-nums">{formatHz(hz)}</span>
        <Button type="button" variant="ghost" size="icon-xs" aria-label="Close" onClick={onClose}>
          ✕
        </Button>
      </div>

      {found.length === 0 && (
        <span className="text-xs text-muted-foreground">
          Nothing allocated here in this region's tables.
        </span>
      )}

      {found.map((entry) => (
        <BandDetail
          key={entry.laneId}
          entry={entry}
          layerName={layerName}
          // Only the winning lane's own stack is worth unrolling; an overlay covers nothing.
          covered={entry.laneId === found[0]?.laneId}
        />
      ))}

      {found.length > 0 && (
        <Button
          type="button"
          size="sm"
          className="mt-0.5 font-mono"
          onClick={() => {
            onTune(hz, suggested);
            onClose();
          }}
        >
          Tune {formatHz(hz)}
          {suggested !== null && ` · ${suggested.type.toUpperCase()}`}
        </Button>
      )}
    </Card>
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
    <div className="flex flex-col gap-0.5 border-t border-border pt-1.5 first:border-t-0 first:pt-0">
      <div className="flex items-center gap-1.5">
        <span
          aria-hidden
          className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
        />
        <span className="min-w-0 truncate text-sm text-foreground">{allocation.name}</span>
        {!allocation.primary && (
          <Badge variant="secondary" title="Must accept interference from every primary service">
            secondary
          </Badge>
        )}
      </div>
      {/* The regulator's own wording, under the operator's. It is the citable one and the one a
          reader checking against the source document will search for — and where the friendly
          name came from an annotation, this is the line that says what was actually allocated. */}
      {allocation.official_name !== allocation.name && (
        <span className="truncate font-mono text-[11px] text-muted-foreground">
          {allocation.official_name}
        </span>
      )}
      <span className={LABEL}>
        {serviceLabel(allocation.service)} · {layerName(allocation.layer)} ·{" "}
        {formatHz(allocation.start_hz)}–{formatHz(allocation.stop_hz)}
        {allocation.reference != null && ` · ${allocation.reference}`}
      </span>
      {allocation.channel_step_hz != null && (
        <Badge variant="secondary">{formatHz(allocation.channel_step_hz)} channels</Badge>
      )}
      {allocation.notes != null && (
        <p className="text-xs leading-snug text-muted-foreground">{allocation.notes}</p>
      )}
      {covered &&
        entry.covered.map((under) => (
          <span key={under.id} className="text-[11px] text-muted-foreground/70">
            {under.layer === allocation.layer ? "also" : `over ${layerName(under.layer)}:`}{" "}
            {under.name}
          </span>
        ))}
    </div>
  );
}
