import { Button } from "../../components/BaseControls";
import { BTN_SM, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { Chips, ChoiceChip, ToggleChip } from "../../components/face/Chips";
import { Readout, Readouts } from "../../components/face/Readouts";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { STITCH_LIMITS } from "../../lib/limits";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type { PatchNode, PatchNodeOf, StitchParams, StitchReading } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { ProcessorError } from "./ProcessorHealth";
import {
  ageLabel,
  NO_CATALOG,
  processorGate,
  processorSubtitle,
  useProcessorEdit,
} from "./processorFace";
import {
  needsSpread,
  SPREAD_CHIP,
  STITCH_BLENDS,
  spreadArrayEdit,
  stitchChips,
  stitchRow,
} from "./stitch";

const AGE_TICK_MS = 1_000;
const STITCH_REPORT_MS = STITCH_LIMITS.report_ms;
const LANE_HEADS = ["Lane", "MHz", "Eq", "Phase", "Coh"] as const;

export function StitchFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const edit = useProcessorEdit(node as PatchNodeOf<"stitch">);
  if (node.kind !== "stitch") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const reading = readingOf(state, "stitch");
  const stale = state !== undefined && isStale(state.receivedAt, now, STITCH_REPORT_MS);
  const processor = processorStatusOf(status, node.id);
  const spread = needsSpread(workspace.graph, node.id, processorGate(status, node.id));
  const spreadArray = () => {
    workspace.edit((snapshot) => {
      const graph = spreadArrayEdit(snapshot.graph, node.id);
      return graph === null ? snapshot : { ...snapshot, graph };
    });
    workspace.apply();
  };
  return (
    <NodeShell
      node={node}
      title="Stitch"
      category="tool"
      subtitle={processorSubtitle(workspace.graph, node.id, status, state, now, STITCH_REPORT_MS)}
    >
      <FaceBody>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <>
            <div className={stale ? "opacity-50" : ""}>
              <Readouts columns={2}>
                <Readout label="Center">{mhz(reading?.center_hz)}</Readout>
                <Readout label="Span">{mhz(reading?.span_hz)}</Readout>
                <Readout label="Lanes">{String(reading?.lanes.length ?? 0)}</Readout>
                <Readout label="Age">{ageLabel(state?.receivedAt, now)}</Readout>
              </Readouts>
              <LaneTable reading={reading} />
            </div>
            <StitchChips settings={settings} edit={edit} />
            <ProcessorError status={processor} />
          </>
        )}
      </FaceBody>
      <ProcessorFooter
        status={processor}
        chips={spread ? [SPREAD_CHIP, ...stitchChips(reading)] : stitchChips(reading)}
        actions={
          spread ? (
            <Button
              type="button"
              className={BTN_SM}
              title="Tune the wired array's lanes side by side"
              onClick={spreadArray}
            >
              Spread array
            </Button>
          ) : undefined
        }
      />
    </NodeShell>
  );
}

function mhz(hz: number | undefined): string {
  return hz === undefined ? "-" : `${(hz / 1e6).toFixed(3)} MHz`;
}

function LaneTable({ reading }: { reading: StitchReading | null }) {
  if (reading === null || reading.lanes.length === 0) {
    return null;
  }
  return (
    <table className="w-full border-t border-line">
      <thead>
        <tr>
          {LANE_HEADS.map((head) => (
            <th key={head} className={TABLE_HEAD}>
              {head}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {reading.lanes.map((lane) => {
          const row = stitchRow(lane);
          return (
            <tr key={lane.lane} title={row.title}>
              <td className={TABLE_CELL}>{row.lane}</td>
              <td className={TABLE_CELL}>{row.mhz}</td>
              <td className={TABLE_CELL}>{row.eq}</td>
              <td className={TABLE_CELL}>{row.phase}</td>
              <td className={TABLE_CELL}>{row.coherence}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function StitchChips({
  settings,
  edit,
}: {
  settings: StitchParams;
  edit: (next: Partial<StitchParams>) => void;
}) {
  return (
    <Chips className="p-2">
      <ChoiceChip
        label="Blend"
        title="Blend"
        value={settings.blend}
        options={STITCH_BLENDS}
        onChange={(blend) => edit({ blend })}
      />
      <ToggleChip
        label="Equalise"
        title="Match noise floors"
        on={settings.noise_equalise}
        onChange={(noise_equalise) => edit({ noise_equalise })}
      />
      <ToggleChip
        label="Flatten"
        title="Undo each lane's filter droop"
        on={settings.flatten}
        onChange={(flatten) => edit({ flatten })}
      />
      <ToggleChip
        label="Spurs"
        title="Drop lane spurs in overlaps"
        on={settings.spur_reject}
        onChange={(spur_reject) => edit({ spur_reject })}
      />
      <ToggleChip
        label="Phase"
        title="Line up phase across seams"
        on={settings.match_phase}
        onChange={(match_phase) => edit({ match_phase })}
      />
    </Chips>
  );
}
