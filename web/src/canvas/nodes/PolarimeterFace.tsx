import { useState } from "react";
import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
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
import { OffsetRow, WidthRow } from "./BandRows";
import { laneOptions } from "./beamformer";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFaults, ProcessorReadout, ReadoutCell } from "./ProcessorReadout";
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
import { SettingsFold } from "./SettingsFold";

const AGE_TICK_MS = 1_000;
const GLYPH_PX = 96;
const SMALL = "w-24";

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
            <div className={`flex gap-3 p-2 ${stale ? "opacity-50" : ""}`}>
              <PolarGlyph reading={reading} />
              <div className="min-w-0 flex-1">
                <PolarReadout reading={reading} receivedAt={state?.receivedAt} now={now} />
              </div>
            </div>
            <ProcessorFaults status={processorStatusOf(status, node.id)} />
            <PolarSettings settings={settings} edit={edit} lanes={lanes} />
          </>
        )}
      </FaceBody>
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
    <ProcessorReadout>
      <ReadoutCell label="I" title="Total power" value={value((r) => `${r.i_db.toFixed(1)} dB`)} />
      <ReadoutCell
        label="Pol"
        title="Polarised share"
        value={value((r) => percentText(r.degree))}
      />
      <ReadoutCell label="Q" value={value((r) => stokesText(r.q))} />
      <ReadoutCell
        label="Tilt"
        title="From horizontal"
        value={value((r) => `${Math.round(r.angle_deg)}°`)}
      />
      <ReadoutCell label="U" value={value((r) => stokesText(r.u))} />
      <ReadoutCell label="Ellip" value={value((r) => `${Math.round(r.ellipticity_deg)}°`)} />
      <ReadoutCell label="V" value={value((r) => stokesText(r.v))} />
      <ReadoutCell label="Hand" value={value((r) => handText(r.hand))} />
      <ReadoutCell
        label="SNR"
        value={value((r) => (r.snr_db == null ? "-" : `${r.snr_db.toFixed(0)} dB`))}
      />
      <ReadoutCell label="Age" value={ageLabel(receivedAt, now)} />
    </ProcessorReadout>
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
      className="shrink-0 rounded-[3px] bg-well"
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

function PolarSettings({
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
      <div className="border-t border-line p-2">
        <Settings>
          <SettingRow label="H" title="Lane of the horizontal antenna">
            <Select
              label="H lane"
              value={settings.h_lane}
              options={laneOptions(lanes)}
              onChange={(lane) => pick("h_lane", lane)}
              className="w-16"
            />
          </SettingRow>
          <SettingRow label="V" title="Lane of the vertical antenna">
            <Select
              label="V lane"
              value={settings.v_lane}
              options={laneOptions(lanes)}
              onChange={(lane) => pick("v_lane", lane)}
              className="w-16"
            />
            {clash && (
              <span role="alert" className="text-xs text-danger">
                {PICK_TWO}
              </span>
            )}
          </SettingRow>
          <SettingRow label="Output" title="What the beam output carries">
            <Segmented
              label="Output"
              value={settings.matched ? "matched" : "cross"}
              options={[
                { value: "matched", label: "Matched", title: "Beam output matched to the wave" },
                { value: "cross", label: "Cross", title: "Beam output across the wave" },
              ]}
              onChange={(output) => edit({ matched: output === "matched" })}
            />
          </SettingRow>
        </Settings>
      </div>
      <SettingsFold label="More">
        <OffsetRow
          limit={LIMITS.band.offset_hz}
          offsetHz={settings.offset_hz}
          onOffset={(offset_hz) => edit({ offset_hz })}
        />
        <WidthRow
          limit={LIMITS.band.bandwidth_hz}
          bandwidthHz={settings.bandwidth_hz}
          onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
        />
        <SettingRow label="Report">
          <NumberField
            label="Report"
            value={settings.report_ms}
            min={LIMITS.report_ms.min}
            max={LIMITS.report_ms.max}
            step={10}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => edit({ report_ms: Math.round(ms) })}
          />
        </SettingRow>
        <SettingRow label="Average">
          <NumberField
            label="Average"
            value={settings.average_ms}
            min={LIMITS.average_ms.min}
            max={LIMITS.average_ms.max}
            step={10}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => edit({ average_ms: Math.round(ms) })}
          />
        </SettingRow>
        <SettingRow label="Fade" title="Blend old and new weights">
          <NumberField
            label="Fade"
            value={settings.crossfade_ms}
            min={LIMITS.crossfade_ms.min}
            max={LIMITS.crossfade_ms.max}
            step={1}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => edit({ crossfade_ms: Math.round(ms) })}
          />
        </SettingRow>
        <SettingRow label="Flip" title="Swap right and left hand">
          <Checkbox
            label="Flip"
            checked={settings.flip_hand}
            onChange={(flip_hand) => edit({ flip_hand })}
          />
        </SettingRow>
      </SettingsFold>
    </>
  );
}
