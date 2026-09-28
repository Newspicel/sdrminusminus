import { Button } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { BTN_SM, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { Segmented } from "../../components/Segmented";
import { SettingRow, Settings } from "../../components/Settings";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type { PatchNode, PatchNodeOf, StitchParams, StitchReading } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorChips, ProcessorFaults, ProcessorReadout, ReadoutCell } from "./ProcessorReadout";
import {
  ageLabel,
  NO_CATALOG,
  processorGate,
  processorSubtitle,
  useProcessorEdit,
} from "./processorFace";
import { needsSpread, STITCH_BLENDS, spreadArrayEdit, stitchChips, stitchRow } from "./stitch";

const AGE_TICK_MS = 1_000;
const STITCH_REPORT_MS = 1_000;
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
            {spread && (
              <div role="alert" className="flex items-center gap-2 px-2 pt-2 text-xs text-danger">
                <span>Needs spread</span>
                <Button
                  type="button"
                  className={`${BTN_SM} ml-auto`}
                  title="Tune the wired array's lanes side by side"
                  onClick={spreadArray}
                >
                  Spread array
                </Button>
              </div>
            )}
            <div className={`flex flex-col gap-2 p-2 ${stale ? "opacity-50" : ""}`}>
              <ProcessorReadout>
                <ReadoutCell label="Center" value={mhz(reading?.center_hz)} />
                <ReadoutCell label="Span" value={mhz(reading?.span_hz)} />
                <ReadoutCell label="Lanes" value={String(reading?.lanes.length ?? 0)} />
                <ReadoutCell label="Age" value={ageLabel(state?.receivedAt, now)} />
              </ProcessorReadout>
              <LaneTable reading={reading} />
            </div>
            <ProcessorChips chips={stitchChips(reading)} />
            <ProcessorFaults status={processorStatusOf(status, node.id)} />
            <StitchSettings settings={settings} edit={edit} />
          </>
        )}
      </FaceBody>
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
    <table className="w-full">
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

function StitchSettings({
  settings,
  edit,
}: {
  settings: StitchParams;
  edit: (next: Partial<StitchParams>) => void;
}) {
  return (
    <div className="border-t border-line p-2">
      <Settings>
        <SettingRow label="Blend">
          <Segmented
            label="Blend"
            value={settings.blend}
            options={STITCH_BLENDS}
            onChange={(blend) => edit({ blend })}
          />
        </SettingRow>
        <SettingRow label="Equalise" title="Match noise floors">
          <Checkbox
            label="Equalise"
            checked={settings.noise_equalise}
            onChange={(noise_equalise) => edit({ noise_equalise })}
          />
        </SettingRow>
        <SettingRow label="Flatten" title="Undo each lane's filter droop">
          <Checkbox
            label="Flatten"
            checked={settings.flatten}
            onChange={(flatten) => edit({ flatten })}
          />
        </SettingRow>
        <SettingRow label="Spurs" title="Drop lane spurs in overlaps">
          <Checkbox
            label="Spurs"
            checked={settings.spur_reject}
            onChange={(spur_reject) => edit({ spur_reject })}
          />
        </SettingRow>
        <SettingRow label="Phase" title="Line up phase across seams">
          <Checkbox
            label="Phase"
            checked={settings.match_phase}
            onChange={(match_phase) => edit({ match_phase })}
          />
        </SettingRow>
      </Settings>
    </div>
  );
}
