import type { Options } from "../../components/controls";
import type {
  PatchGraph,
  ProcessorGate,
  StitchBlend,
  StitchLane,
  StitchReading,
} from "../../lib/types";
import { arrayOf } from "../binding";
import { nodeOf, patchNode } from "../graph";
import type { Chip } from "./ProcessorHealth";

export const STITCH_BLENDS: Options<StitchBlend> = [
  { value: "snr", label: "SNR", title: "Favour the cleaner lane in overlaps" },
  { value: "equal", label: "Equal", title: "Average overlaps" },
];

export const SPREAD_CHIP: Chip = {
  label: "Needs spread",
  title: "Stitch needs the array's lanes tuned side by side",
  danger: true,
};

export function spreadArrayEdit(graph: PatchGraph, stitch: string): PatchGraph | null {
  const array = arrayOf(graph, stitch);
  if (array === null) {
    return null;
  }
  return patchNode(graph, array, (node) =>
    node.kind === "array" ? { ...node, data: { ...node.data, tuning: "spread" } } : node,
  );
}

export function needsSpread(
  graph: PatchGraph,
  stitch: string,
  gate: ProcessorGate | null,
): boolean {
  if (gate === "tuning_mode") {
    return true;
  }
  const array = arrayOf(graph, stitch);
  const node = array === null ? undefined : nodeOf(graph, array);
  return node?.kind === "array" && (node.data.tuning ?? "together") !== "spread";
}

export interface StitchRow {
  lane: string;
  mhz: string;
  eq: string;
  phase: string;
  coherence: string;
  title: string;
}

function signedDb(db: number): string {
  const text = db.toFixed(1);
  return db > 0 && Number(text) !== 0 ? `+${text}` : text;
}

export function stitchRow(lane: StitchLane): StitchRow {
  const coherence = lane.coherence == null ? "-" : lane.coherence.toFixed(2);
  const phase = `${Math.round(lane.phase_deg ?? 0)}°`;
  const spurs = lane.spur_bins ?? 0;
  const row = {
    lane: `L${lane.lane + 1}`,
    mhz: (lane.center_hz / 1e6).toFixed(3),
    eq: `${signedDb(lane.noise_eq_db)} dB`,
    phase,
    coherence,
  };
  return {
    ...row,
    title: [row.lane, row.eq, phase, coherence, ...(spurs > 0 ? [`${spurs} spur bins`] : [])].join(
      " ",
    ),
  };
}

export function stitchChips(reading: StitchReading | null): Chip[] {
  if (reading === null) {
    return [];
  }
  const chips: Chip[] = [];
  if (reading.no_overlap === true) {
    chips.push({ label: "No overlap", title: "Lanes leave gaps between them" });
  }
  const dropped = reading.dropped_blocks ?? 0;
  if (dropped > 0) {
    chips.push({ label: `Drops ${dropped}`, title: "Blocks lost", danger: true });
  }
  return chips;
}
