import { Tooltip } from "@base-ui/react/tooltip";
import { memo, useMemo, useRef, useState } from "react";
import { Button } from "../../components/BaseControls";
import type { BandIdentity, BandSpan } from "../../components/bandPlan";
import {
  coveredByLayer,
  identify,
  provisionText,
  serviceEdge,
  serviceFill,
  serviceLabel,
  spansIn,
  suggestedAt,
} from "../../components/bandPlan";
import { CHIP_SM, LABEL, SURFACE } from "../../components/controls";
import { formatHz } from "../../components/format";
import { usePortalContainer } from "../../components/PortalContainer";
import { type SpectrumView, spanToOffset, viewWidth } from "../../components/spectrumView";
import type { BandAllocation, BandLane, BandPlan, ChannelParams } from "../../lib/types";
import { useBandPlan } from "../../lib/useBandPlan";

const ROW_H = 16;
const LABEL_MIN = 0.07;
const TIP_DELAY_MS = 120;
const META = "block font-mono text-[10px] leading-snug tracking-[0.09em] uppercase text-ink-faint";

export const BandRuler = memo(function BandRuler({
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
  const portalContainer = usePortalContainer();
  const rulerRef = useRef<HTMLDivElement>(null);
  const [hoverAt, setHoverAt] = useState<number | null>(null);

  const visibleHz = spanHz * viewWidth(view);
  const lowHz = centerHz + spanToOffset(view.start, spanHz);
  const lanes = useMemo(
    () =>
      plan === null
        ? []
        : plan.lanes.map((lane) => ({ lane, spans: spansIn(plan, lane, lowHz, visibleHz) })),
    [plan, lowHz, visibleHz],
  );

  if (!ruler || plan === null || !(spanHz > 0)) {
    return null;
  }

  const hzAt = (fraction: number): number => lowHz + fraction * visibleHz;
  const fractionAt = (clientX: number): number | null => {
    const rect = rulerRef.current?.getBoundingClientRect();
    if (rect === undefined || rect.width === 0) {
      return null;
    }
    return Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
  };

  const tuneAt = (event: React.MouseEvent<HTMLElement>): void => {
    event.stopPropagation();
    const at = event.detail === 0 ? null : fractionAt(event.clientX);
    if (at === null) {
      return;
    }
    const hz = Math.round(hzAt(at));
    onTune(hz, suggestedAt(identify(plan, hz)));
  };

  return (
    <Tooltip.Root trackCursorAxis="x" disableHoverablePopup>
      <Tooltip.Trigger
        render={<div ref={rulerRef} />}
        delay={TIP_DELAY_MS}
        data-plot-chrome
        className="relative shrink-0 bg-bg"
        onPointerEnter={(event) => setHoverAt(fractionAt(event.clientX))}
        onPointerMove={(event) => setHoverAt(fractionAt(event.clientX))}
      >
        {lanes.map(({ lane, spans }) => (
          <Lane key={lane.id} lane={lane} spans={spans} onTune={tuneAt} />
        ))}
      </Tooltip.Trigger>
      <Tooltip.Portal container={portalContainer} className="contents">
        <Tooltip.Positioner
          className="z-30 nodrag nopan"
          side="bottom"
          sideOffset={6}
          collisionPadding={8}
        >
          <Tooltip.Popup className={`${SURFACE} w-72 max-w-[calc(100vw-1rem)] p-2 text-left`}>
            {hoverAt !== null && <IdentifyTip plan={plan} hz={hzAt(hoverAt)} />}
          </Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
});

function Lane({
  lane,
  spans,
  onTune,
}: {
  lane: BandLane;
  spans: readonly BandSpan[];
  onTune: (event: React.MouseEvent<HTMLElement>) => void;
}) {
  return (
    <Button
      type="button"
      className="relative block w-full cursor-pointer border-b border-line/60 last:border-b-0"
      style={{ height: `${ROW_H}px` }}
      onClick={onTune}
      onPointerDown={(event) => event.stopPropagation()}
      aria-label={`${lane.name} — hover to identify, click to tune`}
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
}

function IdentifyTip({ plan, hz }: { plan: BandPlan; hz: number }) {
  const found = identify(plan, hz);
  const suggested = suggestedAt(found);
  const layerName = (id: string): string =>
    plan.layers.find((layer) => layer.id === id)?.authority ?? id;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-2">
        <span className="font-mono text-sm text-ink tabular-nums">{formatHz(hz)}</span>
        <span className={LABEL}>click to tune{suggested !== null && ` · ${suggested.type}`}</span>
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
          provision={(layer, id) => provisionText(plan, layer, id)}
          covered={entry.laneId === found[0]?.laneId}
        />
      ))}
    </div>
  );
}

function metaLine(allocation: BandAllocation, layerName: (id: string) => string): string {
  const parts = [
    serviceLabel(allocation.service),
    layerName(allocation.layer),
    `${formatHz(allocation.start_hz)}–${formatHz(allocation.stop_hz)}`,
  ];
  if (
    allocation.reference != null &&
    allocation.reference !== allocation.name &&
    allocation.reference !== allocation.official_name
  ) {
    parts.push(allocation.reference);
  }
  if (allocation.channel_step_hz != null) {
    parts.push(`${formatHz(allocation.channel_step_hz)} steps`);
  }
  return parts.join(" · ");
}

function BandDetail({
  entry,
  layerName,
  provision,
  covered,
}: {
  entry: BandIdentity;
  layerName: (id: string) => string;
  provision: (layer: string, id: string) => string | null;
  covered: boolean;
}) {
  const { allocation } = entry;
  return (
    <div className="flex flex-col gap-0.5 border-t border-line pt-1.5 first:border-t-0 first:pt-0">
      <div className="flex items-start gap-1.5">
        <span
          aria-hidden
          className={`mt-1.5 size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
        />
        <span className="min-w-0 flex-1 text-sm leading-snug text-ink">{allocation.name}</span>
        {!allocation.primary && (
          <span
            className={`${LABEL} shrink-0 pt-0.5`}
            title="Must accept interference from every primary service"
          >
            secondary
          </span>
        )}
      </div>
      {allocation.official_name !== allocation.name && (
        <span className="font-mono text-[11px] leading-snug text-ink-dim">
          {allocation.official_name}
        </span>
      )}
      <span className={META}>{metaLine(allocation, layerName)}</span>
      {allocation.notes != null && (
        <p className="line-clamp-2 text-xs leading-snug text-ink-dim">{allocation.notes}</p>
      )}
      {allocation.provisions != null && allocation.provisions.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {allocation.provisions.map((id) => (
            <span
              key={id}
              className={CHIP_SM}
              title={
                provision(allocation.layer, id) ?? "Not in the cited Frequenzverordnung extract"
              }
            >
              {id}
            </span>
          ))}
        </div>
      )}
      {covered &&
        coveredByLayer(entry.covered, (layer) =>
          layer === allocation.layer ? "also" : `over ${layerName(layer)}`,
        ).map((group) => (
          <span key={group.label} className="text-[11px] leading-snug text-ink-faint">
            {group.label}: {group.names.join(" · ")}
          </span>
        ))}
    </div>
  );
}
