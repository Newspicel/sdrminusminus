import { useState } from "react";
import type { Options } from "../../components/controls";
import { Chips, ChoiceChip, NumberChip, ToggleChip } from "../../components/face/Chips";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { POLARIMETER_LIMITS as LIMITS } from "../../lib/limits";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type {
  PatchNode,
  PatchNodeOf,
  PolarimeterParams,
  PolarimeterReading,
} from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { OffsetChip, WidthChip } from "./BandRows";
import { laneOptions } from "./beamformer";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { ProcessorError } from "./ProcessorHealth";
import {
  ellipsePath,
  handText,
  laneEdit,
  PICK_TWO,
  percentText,
  senseArrow,
  stokesText,
} from "./polarimeter";
import {
  ageLabel,
  NO_CATALOG,
  processorLanes,
  processorSubtitle,
  useProcessorEdit,
} from "./processorFace";

const AGE_TICK_MS = 1_000;
const GLYPH_PX = 96;
const OUTPUT_OPTIONS: Options<"matched" | "cross"> = [
  { value: "matched", label: "Matched", title: "Beam output matched to the wave" },
  { value: "cross", label: "Cross", title: "Beam output across the wave" },
];

type Edit = (next: Partial<PolarimeterParams>) => void;

export function PolarimeterFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const edit = useProcessorEdit(node as PatchNodeOf<"polarimeter">);
  if (node.kind !== "polarimeter") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const reading = readingOf(state, "polarimeter");
  const period = settings?.report_ms ?? 0;
  const stale = state !== undefined && isStale(state.receivedAt, now, period);
  const lanes = Math.max(processorLanes(workspace.graph, array, status), 2);
  const processor = processorStatusOf(status, node.id);
  return (
    <NodeShell
      node={node}
      title="Polarimeter"
      category="tool"
      subtitle={processorSubtitle(workspace.graph, node.id, status, state, now, period)}
    >
      <FaceBody>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <>
            <div className={`flex items-center gap-1 pl-2 ${stale ? "opacity-50" : ""}`}>
              <PolarGlyph reading={reading} />
              <PolarReadout reading={reading} receivedAt={state?.receivedAt} now={now} />
            </div>
            <PolarChips settings={settings} edit={edit} lanes={lanes} />
            <ProcessorError status={processor} />
          </>
        )}
      </FaceBody>
      <ProcessorFooter status={processor} />
    </NodeShell>
  );
}

function PolarReadout({
  reading,
  receivedAt,
  now,
}: {
  reading: PolarimeterReading | null;
  receivedAt: number | undefined;
  now: number;
}) {
  const value = (text: (reading: PolarimeterReading) => string) =>
    reading === null ? "-" : text(reading);
  return (
    <Readouts columns={2} ruled={false} className="min-w-0 flex-1">
      <Readout label="I" title="Total power">
        {value((r) => `${r.i_db.toFixed(1)} dB`)}
      </Readout>
      <Readout label="Pol" title="Polarised share">
        {value((r) => percentText(r.degree))}
      </Readout>
      <Readout label="Q">{value((r) => stokesText(r.q))}</Readout>
      <Readout label="Tilt" title="From horizontal">
        {value((r) => `${Math.round(r.angle_deg)}°`)}
      </Readout>
      <Readout label="U">{value((r) => stokesText(r.u))}</Readout>
      <Readout label="Ellip">{value((r) => `${Math.round(r.ellipticity_deg)}°`)}</Readout>
      <Readout label="V">{value((r) => stokesText(r.v))}</Readout>
      <Readout label="Hand">{value((r) => handText(r.hand))}</Readout>
      <Readout label="SNR">
        {value((r) => (r.snr_db == null ? "-" : `${r.snr_db.toFixed(0)} dB`))}
      </Readout>
      <Readout label="Age">{ageLabel(receivedAt, now)}</Readout>
    </Readouts>
  );
}

function PolarGlyph({ reading }: { reading: PolarimeterReading | null }) {
  const centre = GLYPH_PX / 2;
  const radius = centre - 8;
  const angle = reading?.angle_deg ?? 0;
  const chi = reading?.ellipticity_deg ?? 0;
  const arrow = reading === null ? null : senseArrow(angle, chi, reading.v, radius, centre);
  return (
    <svg
      role="img"
      aria-label="Polarisation"
      width={GLYPH_PX}
      height={GLYPH_PX}
      viewBox={`0 0 ${GLYPH_PX} ${GLYPH_PX}`}
      className="my-2 shrink-0 rounded-[3px] bg-well"
    >
      <path
        d={`M4 ${centre}H${GLYPH_PX - 4}M${centre} 4V${GLYPH_PX - 4}`}
        className="stroke-line"
      />
      <text
        x={GLYPH_PX - 4}
        y={centre - 3}
        textAnchor="end"
        className="fill-ink-faint font-mono text-[8px]"
      >
        H
      </text>
      <text x={centre + 3} y={10} className="fill-ink-faint font-mono text-[8px]">
        V
      </text>
      {reading !== null && (
        <>
          <path
            d={ellipsePath(angle, chi, radius, centre)}
            className="fill-accent/15 stroke-accent"
            strokeWidth={1.5}
          />
          {arrow !== null && (
            <>
              <path d={arrow.arc} className="fill-none stroke-accent" strokeWidth={2.5} />
              <path d={arrow.head} className="fill-accent stroke-accent" strokeLinejoin="round" />
            </>
          )}
        </>
      )}
    </svg>
  );
}

function PolarChips({
  settings,
  edit,
  lanes,
}: {
  settings: PolarimeterParams;
  edit: Edit;
  lanes: number;
}) {
  const [clash, setClash] = useState(false);
  const pick = (which: "h_lane" | "v_lane", lane: number) => {
    const next = laneEdit(settings, which, lane);
    setClash(next === null);
    if (next !== null) {
      edit(next);
    }
  };
  return (
    <>
      <Chips className="p-2">
        <ChoiceChip
          label="H"
          title="Lane of the horizontal antenna"
          value={settings.h_lane}
          options={laneOptions(lanes)}
          onChange={(lane) => pick("h_lane", lane)}
        />
        <ChoiceChip
          label="V"
          title="Lane of the vertical antenna"
          value={settings.v_lane}
          options={laneOptions(lanes)}
          onChange={(lane) => pick("v_lane", lane)}
        />
        <ChoiceChip
          label="Output"
          title="What the beam output carries"
          value={settings.matched ? "matched" : "cross"}
          options={OUTPUT_OPTIONS}
          onChange={(output) => edit({ matched: output === "matched" })}
        />
        <OffsetChip
          limit={LIMITS.band.offset_hz}
          offsetHz={settings.offset_hz}
          onOffset={(offset_hz) => edit({ offset_hz })}
        />
        <WidthChip
          limit={LIMITS.band.bandwidth_hz}
          bandwidthHz={settings.bandwidth_hz}
          onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
        />
        <NumberChip
          label="Report"
          title="Report"
          unit="ms"
          value={settings.report_ms}
          min={LIMITS.report_ms.min}
          max={LIMITS.report_ms.max}
          step={10}
          onCommit={(ms) => edit({ report_ms: Math.round(ms) })}
        />
        <NumberChip
          label="Average"
          title="Average"
          unit="ms"
          value={settings.average_ms}
          min={LIMITS.average_ms.min}
          max={LIMITS.average_ms.max}
          step={10}
          onCommit={(ms) => edit({ average_ms: Math.round(ms) })}
        />
        <NumberChip
          label="Fade"
          title="Blend old and new weights"
          unit="ms"
          value={settings.crossfade_ms}
          min={LIMITS.crossfade_ms.min}
          max={LIMITS.crossfade_ms.max}
          step={1}
          onCommit={(ms) => edit({ crossfade_ms: Math.round(ms) })}
        />
        <ToggleChip
          label="Flip"
          title="Swap right and left hand"
          on={settings.flip_hand}
          onChange={(flip_hand) => edit({ flip_hand })}
        />
      </Chips>
      {clash && <FaceFault message={PICK_TWO} />}
    </>
  );
}
