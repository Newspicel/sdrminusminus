// The band ruler (FEATURES §5, DESIGN.md §9a): a gutter of coloured allocation blocks above the
// trace, aligned to the same frequency axis, with click-to-identify and one-click tune.
//
// It sits *outside* the plot rectangle on purpose. Inside it the colormap owns the whole colour
// budget and every overlay is achromatic (DESIGN.md §2), which a ruler whose entire job is to
// distinguish ten services by sight cannot obey. Moving it into its own opaque strip is the same
// licence the marker label chip already has, and it costs the trace no pixels of data.
//
// It is also chrome as far as the plot's gestures are concerned (`data-plot-chrome`), so a click
// here identifies a band and never retunes a running radio by accident. Tuning is the explicit
// button in the popover.
import { useEffect, useRef, useState } from "react";
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

  const onPick = (event: React.MouseEvent<HTMLElement>): void => {
    const rect = rulerRef.current?.getBoundingClientRect();
    if (rect === undefined || rect.width === 0) {
      return;
    }
    const at = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    setPicked({ hz: hzAt(at), at });
  };

  return (
    <div ref={rulerRef} data-plot-chrome className="relative shrink-0 bg-bg">
      {plan.lanes.map((lane) => {
        const spans = spansIn(lane, lowHz, visibleHz);
        return (
          <button
            key={lane.id}
            type="button"
            className="relative block w-full cursor-help border-b border-line/60 last:border-b-0"
            style={{ height: `${ROW_H}px` }}
            // React Flow selects the node a click landed in and would take the focus straight
            // back off the popover this click just opened.
            onClick={(event) => {
              event.stopPropagation();
              onPick(event);
            }}
            onPointerDown={(event) => event.stopPropagation()}
            aria-label={`${lane.name} — click to identify a frequency`}
          >
            {spans.map((span) => (
              <span
                key={`${span.block.allocation.id}:${span.block.start_hz}`}
                aria-hidden
                className={`absolute inset-y-0 overflow-hidden ${serviceFill(
                  span.block.allocation.service,
                )}`}
                style={{ left: `${span.left * 100}%`, width: `${span.width * 100}%` }}
              >
                {span.startsInside && (
                  <span
                    className={`absolute inset-y-0 left-0 w-px ${serviceEdge(
                      span.block.allocation.service,
                    )}`}
                  />
                )}
                {span.width >= LABEL_MIN && (
                  <span className="absolute inset-y-0 left-1 flex items-center whitespace-nowrap font-mono text-[10px] text-ink">
                    {span.block.allocation.name}
                  </span>
                )}
              </span>
            ))}
          </button>
        );
      })}

      {picked !== null && (
        <IdentifyCard
          hz={picked.hz}
          at={picked.at}
          found={identify(plan, picked.hz)}
          layerName={(id) => plan.layers.find((layer) => layer.id === id)?.authority ?? id}
          onTune={onTune}
          onClose={() => setPicked(null)}
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
  return (
    <div
      className={`absolute top-full z-20 mt-1 flex w-64 -translate-x-1/2 flex-col gap-1.5 p-2 ${SURFACE}`}
      style={{ left: `clamp(8rem, ${at * 100}%, calc(100% - 8rem))` }}
      onKeyDown={(event) => event.key === "Escape" && onClose()}
      role="dialog"
      aria-label={`Allocation at ${formatHz(hz)}`}
    >
      <div className="flex items-baseline justify-between gap-2">
        <span className="font-mono text-sm text-ink tabular-nums">{formatHz(hz)}</span>
        <button
          type="button"
          className="text-ink-faint hover:text-ink"
          aria-label="Close"
          onClick={onClose}
        >
          ✕
        </button>
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
          // Only the winning lane's own stack is worth unrolling; an overlay covers nothing.
          covered={entry.laneId === found[0]?.laneId}
        />
      ))}

      {found.length > 0 && (
        <button
          type="button"
          className="mt-0.5 h-7 rounded-[3px] border border-accent bg-accent/12 px-2 font-mono text-xs text-accent hover:bg-accent/20"
          onClick={() => {
            onTune(hz, suggested);
            onClose();
          }}
        >
          Tune {formatHz(hz)}
          {suggested !== null && ` · ${suggested.type.toUpperCase()}`}
        </button>
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
  const { allocation } = entry.block;
  return (
    <div className="flex flex-col gap-0.5 border-t border-line pt-1.5 first:border-t-0 first:pt-0">
      <div className="flex items-center gap-1.5">
        <span
          aria-hidden
          className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
        />
        <span className="min-w-0 truncate text-sm text-ink">{allocation.name}</span>
      </div>
      <span className={LABEL}>
        {serviceLabel(allocation.service)} · {layerName(allocation.layer)} ·{" "}
        {formatHz(allocation.start_hz)}–{formatHz(allocation.stop_hz)}
      </span>
      {allocation.channel_step_hz != null && (
        <span className={CHIP}>{formatHz(allocation.channel_step_hz)} channels</span>
      )}
      {allocation.notes != null && (
        <p className="text-xs leading-snug text-ink-dim">{allocation.notes}</p>
      )}
      {covered &&
        (entry.block.covered ?? []).map((under) => (
          <span key={under.id} className="text-[11px] text-ink-faint">
            over {layerName(under.layer)}: {under.name}
          </span>
        ))}
    </div>
  );
}
