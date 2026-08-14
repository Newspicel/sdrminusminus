import type {
  BandAllocation,
  BandBlock,
  BandLane,
  BandPlan,
  BandService,
  ChannelParams,
} from "../lib/types";

/** A block clipped to the visible window, in screen fractions. */
export interface BandSpan {
  block: BandBlock;
  allocation: BandAllocation;
  /** Left edge as a fraction of the window; already clamped into [0, 1]. */
  left: number;
  width: number;
  /** Whether the block's own start/stop are inside the window, so the ruler knows which of its
   * edges are real band edges and which are just where the screen ran out. */
  startsInside: boolean;
  endsInside: boolean;
}

/** One search result: the named allocation, and which lane it was found in. */
export interface BandMatch {
  laneId: string;
  laneName: string;
  allocation: BandAllocation;
}

/** What is at a frequency, one entry per lane that has anything to say. */
export interface BandIdentity {
  laneId: string;
  laneName: string;
  block: BandBlock;
  /** The winner over that block. */
  allocation: BandAllocation;
  /** Everything else covering it — co-allocations from the same layer first, then the layers
   * underneath. A regulator routinely gives one range to several services, so this is not an
   * edge case: it is most of the spectrum. */
  covered: BandAllocation[];
}

/** Blocks of `lane` that intersect the window, clipped to it and ordered by frequency.
 *
 * Linear rather than a binary search on purpose: a lane is a few hundred blocks and this runs
 * once per render, not per frame — the render is what is throttled, and a bisect here would buy
 * microseconds at the cost of an off-by-one nobody would notice until a band went missing. */
export function spansIn(
  plan: BandPlan,
  lane: BandLane,
  lowHz: number,
  visibleHz: number,
): BandSpan[] {
  if (!(visibleHz > 0)) {
    return [];
  }
  const highHz = lowHz + visibleHz;
  const spans: BandSpan[] = [];
  for (const block of lane.blocks) {
    if (block.stop_hz <= lowHz || block.start_hz >= highHz) {
      continue;
    }
    const allocation = plan.allocations[block.of];
    if (allocation === undefined) {
      continue;
    }
    const left = (block.start_hz - lowHz) / visibleHz;
    const right = (block.stop_hz - lowHz) / visibleHz;
    spans.push({
      block,
      allocation,
      left: Math.max(0, left),
      width: Math.min(1, right) - Math.max(0, left),
      startsInside: left >= 0,
      endsInside: right <= 1,
    });
  }
  return spans;
}

export function identify(plan: BandPlan, hz: number): BandIdentity[] {
  const found: BandIdentity[] = [];
  for (const lane of plan.lanes) {
    const block = lane.blocks.find((entry) => entry.start_hz <= hz && entry.stop_hz > hz);
    const allocation = block === undefined ? undefined : plan.allocations[block.of];
    if (block === undefined || allocation === undefined) {
      continue;
    }
    found.push({
      laneId: lane.id,
      laneName: lane.name,
      block,
      allocation,
      covered: (block.covered ?? [])
        .map((at) => plan.allocations[at])
        .filter((entry): entry is BandAllocation => entry !== undefined),
    });
  }
  return found;
}

export function suggestedAt(found: readonly BandIdentity[]): ChannelParams | null {
  let suggested: ChannelParams | null = null;
  for (const entry of found) {
    suggested = entry.allocation.suggested ?? suggested;
  }
  return suggested;
}

export function searchPlan(plan: BandPlan, query: string, limit = 40): BandMatch[] {
  const words = query
    .toLowerCase()
    .split(/[\s,]+/)
    .filter((word) => word.length >= 2);
  const hz = parseFrequency(query);
  if (words.length === 0 && hz === null) {
    return [];
  }

  const lanes = new Map<string, string>();
  for (const lane of plan.lanes) {
    for (const block of lane.blocks) {
      for (const at of [block.of, ...(block.covered ?? [])]) {
        const allocation = plan.allocations[at];
        if (allocation !== undefined && !lanes.has(allocation.id)) {
          lanes.set(allocation.id, lane.id === "allocation" ? "" : lane.name);
        }
      }
    }
  }

  const scored: { match: BandMatch; score: number; width: number }[] = [];
  for (const allocation of plan.allocations) {
    const covers = hz !== null && allocation.start_hz <= hz && allocation.stop_hz > hz;
    const haystack = haystackOf(allocation);
    const matched = words.filter((word) => haystack.includes(word)).length;
    if (!covers && matched === 0) {
      continue;
    }
    scored.push({
      score: (covers ? 100 : 0) + matched,
      width: allocation.stop_hz - allocation.start_hz,
      match: {
        laneId: allocation.layer,
        laneName: lanes.get(allocation.id) ?? "",
        allocation,
      },
    });
  }
  scored.sort((a, b) => b.score - a.score || a.width - b.width);
  return scored.slice(0, limit).map((entry) => entry.match);
}

/** A frequency written the way an operator says it: "145.5", "433 MHz", "77.5 kHz", "1.09 GHz".
 * A bare number is megahertz, which is the unit the dial reads in. Returns `null` for anything
 * that is not just a number and an optional unit — "20 m" is a band name, not 20 metres of
 * frequency. */
export function parseFrequency(query: string): number | null {
  const match = /^\s*(\d+(?:[.,]\d+)?)\s*(ghz|mhz|khz|hz)?\s*$/i.exec(query);
  if (match?.[1] === undefined) {
    return null;
  }
  const value = Number.parseFloat(match[1].replace(",", "."));
  if (!Number.isFinite(value)) {
    return null;
  }
  switch (match[2]?.toLowerCase()) {
    case "ghz":
      return value * 1e9;
    case "khz":
      return value * 1e3;
    case "hz":
      return value;
    default:
      return value * 1e6;
  }
}

/** A block's body: its service hue at a quarter, so the ruler reads as tinted ground and the
 * name written across it keeps contrast against the app's own background. Written out per
 * service rather than composed, because Tailwind only emits classes it can literally see —
 * `bg-band-${service}` would compile to nothing at all. */
const FILL: Record<BandService, string> = {
  amateur: "bg-band-amateur/25",
  broadcast: "bg-band-broadcast/25",
  aeronautical: "bg-band-aeronautical/25",
  maritime: "bg-band-maritime/25",
  mobile: "bg-band-mobile/25",
  satellite: "bg-band-satellite/25",
  navigation: "bg-band-navigation/25",
  science: "bg-band-science/25",
  ism: "bg-band-ism/25",
  other: "bg-band-other/25",
};

/** A real band edge, at full chroma. Only drawn where the band actually starts — an edge at the
 * window's rim would claim a boundary that is only where the screen ran out. */
const EDGE: Record<BandService, string> = {
  amateur: "bg-band-amateur",
  broadcast: "bg-band-broadcast",
  aeronautical: "bg-band-aeronautical",
  maritime: "bg-band-maritime",
  mobile: "bg-band-mobile",
  satellite: "bg-band-satellite",
  navigation: "bg-band-navigation",
  science: "bg-band-science",
  ism: "bg-band-ism",
  other: "bg-band-other",
};

export function serviceFill(service: BandService): string {
  return FILL[service];
}

export function serviceEdge(service: BandService): string {
  return EDGE[service];
}

export function serviceLabel(service: BandService): string {
  return service === "ism" ? "ISM" : service.charAt(0).toUpperCase() + service.slice(1);
}

function haystackOf(allocation: BandAllocation): string {
  // "ham" is not an alias anyone would author on fifty amateur rows, and it is what half the
  // world calls the service.
  const service = allocation.service === "amateur" ? "amateur ham" : allocation.service;
  return `${allocation.name} ${(allocation.aliases ?? []).join(" ")} ${service}`.toLowerCase();
}
