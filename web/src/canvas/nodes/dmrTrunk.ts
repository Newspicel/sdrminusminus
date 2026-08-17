import type {
  DmrChannelEntry,
  DmrSearchRange,
  DmrTrunkProtocol,
  DvTrunkProtocol,
  TrunkChannel,
  TrunkChannelSource,
} from "../../lib/types";

export const MAX_SEARCH_RANGES = 8;
export const MAX_SEARCH_CANDIDATES = 512;
export const MIN_SEARCH_STEP_HZ = 1_250;
export const MAX_CHANNEL_MAP = 512;
export const MAX_LOGICAL_CHANNEL = 4095;

export const DMR_TRUNK_PROTOCOLS: readonly { value: DmrTrunkProtocol; label: string }[] = [
  { value: "auto", label: "Auto-detect" },
  { value: "capacity_plus", label: "Capacity Plus" },
  { value: "hytera_xpt", label: "Hytera XPT" },
  { value: "tier_three", label: "Tier III / Capacity Max" },
];

export function dmrTrunkGuidance(
  protocol: DmrTrunkProtocol,
  detected: DvTrunkProtocol | null = null,
): string {
  if (protocol === "auto" && detected !== null) {
    switch (detected) {
      case "capacity_plus":
        return "Detected Capacity Plus signalling; both timeslots of every wired carrier are being followed.";
      case "hytera_xpt":
        return "Detected Hytera XPT signalling; both timeslots of every wired carrier are being followed.";
      case "tier_three":
        return "Detected Tier III signalling; voice grants create traffic receivers automatically.";
    }
  }
  switch (protocol) {
    case "capacity_plus":
      return "Add one DMR decoder for every known repeater output frequency. Both timeslots are isolated automatically.";
    case "hytera_xpt":
      return "Add one DMR decoder for every Hytera XPT repeater output frequency. Both timeslots are isolated automatically.";
    case "tier_three":
      return "Add the DMR control-channel decoder. Standard channel definitions and voice grants create traffic receivers automatically.";
    case "auto":
      return "The system detects Capacity Plus, Hytera XPT, or Tier III signalling from the connected DMR carriers.";
  }
}

export function followsTierThree(
  protocol: DmrTrunkProtocol,
  detected: DvTrunkProtocol | null = null,
): boolean {
  return protocol === "tier_three" || (protocol === "auto" && detected === "tier_three");
}

function megahertz(value: number): string {
  return (value / 1e6).toFixed(4).replace(/\.?0+$/, "");
}

function lines(text: string): string[] {
  return text.split(/[\n;,]/);
}

export function parseSearchRanges(text: string): DmrSearchRange[] {
  const ranges: DmrSearchRange[] = [];
  for (const line of lines(text)) {
    const match = /^\s*([\d.]+)\s*[-–]\s*([\d.]+)\s*\/\s*([\d.]+)\s*$/.exec(line);
    if (match === null) {
      continue;
    }
    const range = {
      start_hz: Math.round(Number(match[1]) * 1e6),
      end_hz: Math.round(Number(match[2]) * 1e6),
      step_hz: Math.round(Number(match[3]) * 1e3),
    };
    if (searchRangeValid(range)) {
      ranges.push(range);
    }
    if (ranges.length >= MAX_SEARCH_RANGES) {
      break;
    }
  }
  return ranges;
}

export function searchRangeValid(range: DmrSearchRange): boolean {
  return (
    Number.isFinite(range.start_hz) &&
    Number.isFinite(range.end_hz) &&
    range.start_hz > 0 &&
    range.end_hz >= range.start_hz &&
    range.step_hz >= MIN_SEARCH_STEP_HZ &&
    searchCandidates([range]) <= MAX_SEARCH_CANDIDATES
  );
}

export function searchCandidates(ranges: readonly DmrSearchRange[]): number {
  return ranges.reduce(
    (total, range) =>
      total +
      (range.step_hz > 0 ? Math.floor((range.end_hz - range.start_hz) / range.step_hz) + 1 : 0),
    0,
  );
}

export function formatSearchRanges(ranges: readonly DmrSearchRange[] | undefined): string {
  return (ranges ?? [])
    .map(
      (range) => `${megahertz(range.start_hz)}-${megahertz(range.end_hz)} / ${range.step_hz / 1e3}`,
    )
    .join("; ");
}

export function parseChannelMap(text: string): DmrChannelEntry[] {
  const seen = new Map<number, number>();
  for (const line of lines(text)) {
    const match = /^\s*(\d+)\s*[=\s]\s*([\d.]+)\s*$/.exec(line);
    if (match === null) {
      continue;
    }
    const lcn = Number(match[1]);
    const freq_hz = Math.round(Number(match[2]) * 1e6);
    if (lcn <= MAX_LOGICAL_CHANNEL && freq_hz > 0 && seen.size < MAX_CHANNEL_MAP) {
      seen.set(lcn, freq_hz);
    }
  }
  return [...seen].map(([lcn, freq_hz]) => ({ lcn, freq_hz })).sort((a, b) => a.lcn - b.lcn);
}

export function formatChannelMap(entries: readonly DmrChannelEntry[] | undefined): string {
  return (entries ?? []).map((entry) => `${entry.lcn} = ${megahertz(entry.freq_hz)}`).join("; ");
}

export function parseControlHz(text: string): number | undefined {
  const value = Number(text.trim());
  if (!Number.isFinite(value) || value <= 0) {
    return undefined;
  }
  return Math.round(value * 1e6);
}

export function trunkChannelSourceLabel(source: TrunkChannelSource): string {
  switch (source) {
    case "announced":
      return "announced";
    case "manual":
      return "entered";
    case "learned":
      return "found";
    case "predicted":
      return "guessed";
  }
}

export function trunkChannelSourceHint(source: TrunkChannelSource): string {
  switch (source) {
    case "announced":
      return "The system broadcast this frequency itself.";
    case "manual":
      return "You entered this frequency.";
    case "learned":
      return "A call answered a grant here, so the frequency is confirmed.";
    case "predicted":
      return "Worked out from the channel spacing of the channels already known. Never followed until a call confirms it.";
  }
}

export function trunkChannelSourceTone(source: TrunkChannelSource): string {
  return source === "predicted" ? "text-ink-faint" : "text-ink-dim";
}

export function usable(source: TrunkChannelSource): boolean {
  return source !== "predicted";
}

export function channelPlanRows(
  map: readonly TrunkChannel[],
  entries: readonly DmrChannelEntry[] | undefined,
): TrunkChannel[] {
  const known = new Map(map.map((channel) => [channel.logical_channel, channel]));
  for (const entry of entries ?? []) {
    if (!known.has(entry.lcn)) {
      known.set(entry.lcn, {
        logical_channel: entry.lcn,
        freq_hz: entry.freq_hz,
        source: "manual",
        confidence: 100,
      });
    }
  }
  return [...known.values()].sort((a, b) => a.logical_channel - b.logical_channel);
}

export function withoutChannel(
  entries: readonly DmrChannelEntry[] | undefined,
  lcn: number,
): DmrChannelEntry[] {
  return (entries ?? []).filter((entry) => entry.lcn !== lcn);
}

export function planSummary(rows: readonly TrunkChannel[]): string {
  if (rows.length === 0) {
    return "No logical channels placed yet.";
  }
  const counted = rows.reduce<Record<string, number>>((totals, row) => {
    totals[row.source] = (totals[row.source] ?? 0) + 1;
    return totals;
  }, {});
  const parts = (["announced", "manual", "learned", "predicted"] as const)
    .filter((source) => counted[source] !== undefined)
    .map((source) => `${counted[source]} ${trunkChannelSourceLabel(source)}`);
  return `${rows.length} logical channel${rows.length === 1 ? "" : "s"} — ${parts.join(", ")}.`;
}

export function adoptable(
  map: readonly TrunkChannel[],
  entries: readonly DmrChannelEntry[] | undefined,
): DmrChannelEntry[] {
  const known = new Set((entries ?? []).map((entry) => entry.lcn));
  return map
    .filter((channel) => channel.source === "learned" && !known.has(channel.logical_channel))
    .map((channel) => ({ lcn: channel.logical_channel, freq_hz: channel.freq_hz }));
}

export function searchSummary(
  ranges: readonly DmrSearchRange[] | undefined,
  searching: number,
  probes: number,
): string {
  const candidates = searchCandidates(ranges ?? []);
  if (candidates === 0) {
    return "Give the search a frequency range to sweep.";
  }
  if (searching === 0) {
    return `${candidates} frequencies ready to sweep. The search starts when a grant names a channel nobody has placed yet.`;
  }
  return `Hunting ${searching} logical channel${searching === 1 ? "" : "s"} across ${candidates} frequencies with ${probes} receiver${probes === 1 ? "" : "s"}.`;
}
