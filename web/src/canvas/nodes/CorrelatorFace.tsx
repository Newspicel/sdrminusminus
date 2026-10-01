import { useState } from "react";
import type { Options } from "../../components/controls";
import { Chips, ChoiceChip, NumberChip, ToggleChip } from "../../components/face/Chips";
import { Readout, Readouts } from "../../components/face/Readouts";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { CORRELATOR_LIMITS as LIMITS } from "../../lib/limits";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type { CorrelatorParams, CorrelatorReading, PatchNode, PatchNodeOf } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { BandChips } from "./BandRows";
import { CorrelatorPlots, FringeLine } from "./CorrelatorPlots";
import {
  BIN_OPTIONS,
  baselineLabel,
  baselineOptions,
  baselineText,
  channelOptions,
  withFringe,
} from "./correlator";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { ProcessorError } from "./ProcessorHealth";
import { ageLabel, NO_CATALOG, processorSubtitle, useProcessorEdit } from "./processorFace";

const AGE_TICK_MS = 1_000;

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
  const processor = processorStatusOf(status, node.id);
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
            <CorrelatorPlots
              node={node.id}
              known={processor !== null}
              index={index}
              dim={stale || reading === null}
            />
            <CorrelatorChips
              settings={settings}
              edit={edit}
              baseline={
                reading === null || reading.baselines.length === 0
                  ? null
                  : { index, options: baselineOptions(reading.baselines), onPick: setChosen }
              }
            />
            <div className="flex shrink-0 items-center gap-2 border-t border-line pr-2">
              <Readouts columns={2} ruled={false} className="min-w-0 flex-1">
                {baseline !== undefined && (
                  <Readout
                    label={baselineLabel(baseline)}
                    title="Coherence, phase, delay, SNR"
                    wide
                  >
                    <span>{baselineText(baseline)}</span>
                  </Readout>
                )}
                <Readout label="Delay" title="From the phase slope">
                  {baseline === undefined ? "-" : `${baseline.delay_ns.toFixed(2)} ns`}
                </Readout>
                <Readout label="Coh">
                  {baseline === undefined ? "-" : baseline.coherence.toFixed(2)}
                </Readout>
                <Readout label="Int">
                  {reading === null ? "-" : `${reading.integrated_s.toFixed(1)} s`}
                </Readout>
                <Readout label="Age">{ageLabel(state?.receivedAt, now)}</Readout>
              </Readouts>
              <FringeLine history={fringe} />
            </div>
            <ProcessorError status={processor} />
          </div>
        )}
      </FaceBody>
      <ProcessorFooter status={processor} />
    </NodeShell>
  );
}

interface BaselinePick {
  index: number;
  options: Options<number>;
  onPick: (index: number) => void;
}

function CorrelatorChips({
  settings,
  edit,
  baseline,
}: {
  settings: CorrelatorParams;
  edit: (next: Partial<CorrelatorParams>) => void;
  baseline: BaselinePick | null;
}) {
  return (
    <Chips className="shrink-0 p-2">
      {baseline !== null && (
        <ChoiceChip
          label="Baseline"
          title="Baseline"
          value={baseline.index}
          options={baseline.options}
          onChange={baseline.onPick}
        />
      )}
      <ChoiceChip
        label="FFT"
        title="Frequency bins"
        value={settings.bins}
        options={BIN_OPTIONS}
        onChange={(bins) => edit({ bins, channels: Math.min(settings.channels, bins) })}
      />
      <ChoiceChip
        label="Channels"
        title="Frequency points sent per baseline"
        value={settings.channels}
        options={channelOptions(settings.bins)}
        onChange={(channels) => edit({ channels })}
      />
      <NumberChip
        label="Integrate"
        title="Averaging time per result"
        unit="s"
        value={settings.integrate_s}
        min={LIMITS.integrate_s.min}
        max={LIMITS.integrate_s.max}
        step={0.05}
        onCommit={(integrate_s) => edit({ integrate_s })}
      />
      <ToggleChip
        label="Overlap"
        title="Half-overlapping FFTs, twice the work"
        on={settings.overlap}
        onChange={(overlap) => edit({ overlap })}
      />
      <BandChips
        band={LIMITS.band}
        offsetHz={settings.offset_hz}
        bandwidthHz={settings.bandwidth_hz ?? null}
        onOffset={(offset_hz) => edit({ offset_hz })}
        onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
      />
    </Chips>
  );
}
