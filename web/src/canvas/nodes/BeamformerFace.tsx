import { MeterBar } from "../../components/face/Meter";
import { Readout, Readouts } from "../../components/face/Readouts";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type { BeamformerReading, PatchNode, PatchNodeOf } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf, hasWire } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { BeamformerSettings } from "./BeamformerSettings";
import {
  amplitudePercent,
  angleGap,
  beamChips,
  modeLabel,
  NULL_CLOSE_DEG,
  outText,
  patternPath,
  polarTick,
  steerLabel,
  weightTitle,
} from "./beamformer";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { ProcessorError } from "./ProcessorHealth";
import {
  ageLabel,
  NO_CATALOG,
  processorLanes,
  processorSubtitle,
  useProcessorEdit,
} from "./processorFace";

const AGE_TICK_MS = 1_000;
const STEER_PORT = "steer";
const PATTERN_PX = 120;

export function BeamformerFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const edit = useProcessorEdit(node as PatchNodeOf<"beamformer">);
  if (node.kind !== "beamformer") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const reading = readingOf(state, "beamformer");
  const period = settings?.update_ms ?? 0;
  const stale = state !== undefined && isStale(state.receivedAt, now, period);
  const health = processorStatusOf(status, node.id);
  const lanes = Math.max(
    processorLanes(workspace.graph, array, status),
    reading?.weights.length ?? 0,
  );
  return (
    <NodeShell
      node={node}
      title="Beamformer"
      category="tool"
      subtitle={processorSubtitle(workspace.graph, node.id, status, state, now, period)}
    >
      <FaceBody>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <>
            <div className={`flex gap-2 p-2 ${stale ? "opacity-50" : ""}`}>
              <BeamPattern reading={reading} />
              <div className="flex min-w-0 flex-1 flex-col gap-2">
                <Readouts ruled={false} padded={false}>
                  <Readout label="Mode">{modeLabel(reading?.mode ?? settings.mode)}</Readout>
                  <Readout label="SNR">
                    {reading?.snr_db == null ? "-" : `${reading.snr_db.toFixed(0)} dB`}
                  </Readout>
                  <Readout label="Gain" title="Gain over one lane">
                    {gainLabel(reading)}
                  </Readout>
                  <Readout label="Steer">
                    {reading === null ? "-" : steerLabel(reading, settings.steer)}
                  </Readout>
                  {reading?.cancelled_db != null && (
                    <Readout
                      label="Cut"
                      title="Removed by the canceller"
                    >{`${reading.cancelled_db.toFixed(0)} dB`}</Readout>
                  )}
                  <Readout label="Level" title="Beam output level">
                    {reading === null ? "-" : `${reading.output_db.toFixed(1)} dB`}
                  </Readout>
                  <Readout label="Age">{ageLabel(state?.receivedAt, now)}</Readout>
                  <Readout label="Out" title="Beam lane centre and rate" wide>
                    {outText(reading)}
                  </Readout>
                </Readouts>
                <WeightBars reading={reading} />
              </div>
            </div>
            <BeamformerSettings
              settings={settings}
              edit={edit}
              lanes={lanes}
              steerWired={hasWire(workspace.graph, node.id, STEER_PORT)}
            />
            <ProcessorError status={health} />
          </>
        )}
      </FaceBody>
      <ProcessorFooter status={health} chips={beamChips(reading)} />
    </NodeShell>
  );
}

function gainLabel(reading: BeamformerReading | null): string {
  const gain = reading?.sinr_gain_db;
  if (gain == null) {
    return "-";
  }
  return `${gain > 0 ? "+" : ""}${gain.toFixed(1)} dB`;
}

function BeamPattern({ reading }: { reading: BeamformerReading | null }) {
  const centre = PATTERN_PX / 2;
  const radius = centre - 6;
  const pattern = reading?.pattern ?? [];
  const steer = reading?.steer_deg ?? null;
  return (
    <svg
      role="img"
      aria-label="Beam pattern"
      width={PATTERN_PX}
      height={PATTERN_PX}
      viewBox={`0 0 ${PATTERN_PX} ${PATTERN_PX}`}
      className="shrink-0"
    >
      <circle cx={centre} cy={centre} r={radius} className="fill-well stroke-line" />
      <circle cx={centre} cy={centre} r={radius / 2} className="fill-none stroke-line/60" />
      <text x={centre} y={9} textAnchor="middle" className="fill-ink-faint font-mono text-[8px]">
        Fwd
      </text>
      {pattern.length > 0 && (
        <path d={patternPath(pattern, radius, centre)} className="fill-accent/20 stroke-accent" />
      )}
      {steer !== null && (
        <path
          d={polarTick(steer, radius - 8, radius + 4, centre)}
          className="stroke-accent"
          strokeWidth={2}
        />
      )}
      {(reading?.nulls_deg ?? []).map((deg) => (
        <path
          key={deg}
          d={polarTick(deg, radius - 6, radius + 4, centre)}
          className={
            steer !== null && angleGap(deg, steer) < NULL_CLOSE_DEG
              ? "stroke-warn"
              : "stroke-danger"
          }
          strokeWidth={1.5}
        />
      ))}
    </svg>
  );
}

function WeightBars({ reading }: { reading: BeamformerReading | null }) {
  const weights = reading?.weights ?? [];
  if (weights.length === 0) {
    return null;
  }
  const percent = amplitudePercent(weights);
  return (
    <div role="group" className="flex flex-col gap-0.5" aria-label="Lane weights">
      {weights.map((weight, lane) => (
        <div key={lane} className="flex items-center gap-1.5" title={weightTitle(lane, weight)}>
          <span className="w-4 font-mono text-[10px] text-port-array">{lane + 1}</span>
          <MeterBar
            label={`Lane ${lane + 1} weight`}
            value={(percent[lane] ?? 0) / 100}
            valueText={weightTitle(lane, weight)}
          />
        </div>
      ))}
    </div>
  );
}
