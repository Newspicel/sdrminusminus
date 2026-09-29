import { useState } from "react";
import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { Select } from "../../components/Select";
import { SettingRow } from "../../components/Settings";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { CORRELATOR_LIMITS as LIMITS } from "../../lib/limits";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type { CorrelatorParams, CorrelatorReading, PatchNode, PatchNodeOf } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { BandRows } from "./BandRows";
import { CorrelatorPlots, FringeLine } from "./CorrelatorPlots";
import {
  BIN_OPTIONS,
  baselineOptions,
  baselineText,
  channelOptions,
  withFringe,
} from "./correlator";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFaults, ProcessorReadout, ReadoutCell } from "./ProcessorReadout";
import { ageLabel, NO_CATALOG, processorSubtitle, useProcessorEdit } from "./processorFace";
import { SettingsFold } from "./SettingsFold";

const AGE_TICK_MS = 1_000;
const SMALL = "w-24";

interface Fringe {
  key: string;
  index: number;
  history: readonly number[];
}

function useFringe(reading: CorrelatorReading | null, index: number): readonly number[] {
  const [fringe, setFringe] = useState<Fringe>({ key: "", index, history: [] });
  const baseline = reading?.baselines[index];
  const key = `${index}:${reading?.at ?? ""}`;
  if (baseline !== undefined && fringe.key !== key) {
    const phase = baseline.phase_deg ?? 0;
    const history = fringe.index === index ? withFringe(fringe.history, phase) : [phase];
    setFringe({ key, index, history });
    return history;
  }
  return fringe.index === index ? fringe.history : [];
}

export function CorrelatorFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const edit = useProcessorEdit(node as PatchNodeOf<"correlator">);
  const [chosen, setChosen] = useState(0);
  const reading = readingOf(state, "correlator");
  const index = chosen < (reading?.baselines.length ?? 0) ? chosen : 0;
  const fringe = useFringe(reading, index);
  if (node.kind !== "correlator") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const period = Math.max(1_000, (settings?.integrate_s ?? 1) * 1_000);
  const stale = state !== undefined && isStale(state.receivedAt, now, period);
  const baseline = reading?.baselines[index];
  return (
    <NodeShell
      node={node}
      title="Correlator"
      category="tool"
      subtitle={processorSubtitle(workspace.graph, node.id, status, state, now, period)}
    >
      <FaceBody scroll={false}>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">
            <div className="flex shrink-0 items-center gap-2 px-2 py-1.5">
              <span className="legend">Baseline</span>
              {baseline === undefined ? (
                <span className="font-mono text-xs text-ink-faint">-</span>
              ) : (
                <Select
                  label="Baseline"
                  value={index}
                  options={baselineOptions(reading?.baselines ?? [])}
                  onChange={setChosen}
                  className="w-20"
                />
              )}
              <span className="ml-auto truncate font-mono text-xs tabular-nums text-ink">
                {baseline === undefined ? "-" : baselineText(baseline)}
              </span>
            </div>
            <CorrelatorPlots
              node={node.id}
              known={processorStatusOf(status, node.id) !== null}
              index={index}
              dim={stale || reading === null}
            />
            <div className="flex shrink-0 items-center gap-2 border-t border-line px-2 py-1.5">
              <div className="min-w-0 flex-1">
                <ProcessorReadout columns={3}>
                  <ReadoutCell
                    label="Delay"
                    title="From the phase slope"
                    value={baseline === undefined ? "-" : `${baseline.delay_ns.toFixed(2)} ns`}
                  />
                  <ReadoutCell
                    label="Coh"
                    value={baseline === undefined ? "-" : baseline.coherence.toFixed(2)}
                  />
                  <ReadoutCell
                    label="Int"
                    value={reading === null ? "-" : `${reading.integrated_s.toFixed(1)} s`}
                  />
                  <ReadoutCell label="Age" value={ageLabel(state?.receivedAt, now)} />
                </ProcessorReadout>
              </div>
              <FringeLine history={fringe} />
            </div>
            <ProcessorFaults status={processorStatusOf(status, node.id)} />
            <div className="max-h-44 shrink-0 overflow-y-auto">
              <CorrelatorSettings settings={settings} edit={edit} />
            </div>
          </div>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function CorrelatorSettings({
  settings,
  edit,
}: {
  settings: CorrelatorParams;
  edit: (next: Partial<CorrelatorParams>) => void;
}) {
  return (
    <SettingsFold label="Settings">
      <SettingRow label="FFT" title="Frequency bins">
        <Select
          label="FFT"
          value={settings.bins}
          options={BIN_OPTIONS}
          onChange={(bins) => edit({ bins, channels: Math.min(settings.channels, bins) })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Channels" title="Frequency points sent per baseline">
        <Select
          label="Channels"
          value={settings.channels}
          options={channelOptions(settings.bins)}
          onChange={(channels) => edit({ channels })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Integrate" title="Averaging time per result">
        <NumberField
          label="Integrate"
          value={settings.integrate_s}
          min={LIMITS.integrate_s.min}
          max={LIMITS.integrate_s.max}
          step={0.05}
          unit="s"
          className={SMALL}
          onCommit={(integrate_s) => edit({ integrate_s })}
        />
      </SettingRow>
      <SettingRow label="Overlap" title="Half-overlapping FFTs, twice the work">
        <Checkbox
          label="Overlap"
          checked={settings.overlap}
          onChange={(overlap) => edit({ overlap })}
        />
      </SettingRow>
      <BandRows
        band={LIMITS.band}
        offsetHz={settings.offset_hz}
        bandwidthHz={settings.bandwidth_hz ?? null}
        onOffset={(offset_hz) => edit({ offset_hz })}
        onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
      />
    </SettingsFold>
  );
}
