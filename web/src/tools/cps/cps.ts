import type {
  Codeplug,
  CodeplugChannel,
  CodeplugContact,
  CodeplugGroupList,
  CodeplugRadioId,
  CodeplugScanList,
  CodeplugZone,
  ConversionIssue,
  ConversionReport,
  CpsJob,
  CpsPort,
  RadioModelDescriptor,
  Tone,
} from "../../lib/types";

export interface CodeplugLists {
  channels: CodeplugChannel[];
  contacts: CodeplugContact[];
  groupLists: CodeplugGroupList[];
  zones: CodeplugZone[];
  scanLists: CodeplugScanList[];
  radioIds: CodeplugRadioId[];
}

export function lists(codeplug: Codeplug): CodeplugLists {
  return {
    channels: codeplug.channels ?? [],
    contacts: codeplug.contacts ?? [],
    groupLists: codeplug.group_lists ?? [],
    zones: codeplug.zones ?? [],
    scanLists: codeplug.scan_lists ?? [],
    radioIds: codeplug.radio_ids ?? [],
  };
}

export function zoneChannels(zone: CodeplugZone): string[] {
  return [...(zone.channels_a ?? []), ...(zone.channels_b ?? [])];
}

export function formatMhz(hz: number): string {
  return (hz / 1_000_000).toFixed(5);
}

export function formatShift(channel: CodeplugChannel): string {
  const shift = channel.tx_hz - channel.rx_hz;
  if (shift === 0) {
    return "simplex";
  }
  const sign = shift > 0 ? "+" : "−";
  return `${sign}${(Math.abs(shift) / 1_000_000).toFixed(4)}`;
}

export function formatTone(tone: Tone | null | undefined): string {
  if (tone === null || tone === undefined) {
    return "—";
  }
  if (tone.kind === "ctcss") {
    return (tone.decihertz / 10).toFixed(1);
  }
  return `D${String(tone.code).padStart(3, "0")}${tone.inverted ? "I" : "N"}`;
}

export function channelKind(channel: CodeplugChannel): "fm" | "dmr" {
  return channel.mode === "dmr" ? "dmr" : "fm";
}

export function channelDetail(channel: CodeplugChannel): string {
  if (channel.mode === "dmr") {
    const slot = channel.time_slot === "two" ? "TS2" : "TS1";
    return `CC${channel.color_code} ${slot} ${channel.contact ?? "—"}`;
  }
  const width = channel.bandwidth === "wide" ? "25 kHz" : "12.5 kHz";
  return `${width}  ${formatTone(channel.rx_tone)} / ${formatTone(channel.tx_tone)}`;
}

export function jobPercent(job: CpsJob): number {
  if (job.total_bytes <= 0) {
    return job.state === "done" ? 100 : 0;
  }
  return Math.min(100, Math.max(0, (job.done_bytes / job.total_bytes) * 100));
}

export function jobIsActive(job: CpsJob): boolean {
  return job.state === "pending" || job.state === "running";
}

export function latestJob(jobs: readonly CpsJob[]): CpsJob | null {
  return jobs.reduce<CpsJob | null>(
    (best, job) => (best === null || job.id > best.id ? job : best),
    null,
  );
}

export function anyActive(jobs: readonly CpsJob[]): boolean {
  return jobs.some(jobIsActive);
}

export function describeJob(job: CpsJob): string {
  const verb = job.kind === "read" ? "Reading" : job.kind === "write" ? "Writing" : "Identifying";
  switch (job.state) {
    case "pending":
    case "running":
      return `${verb} · ${job.step} · ${jobPercent(job).toFixed(0)}%`;
    case "done":
      return `${verb} finished`;
    case "cancelled":
      return `${verb} cancelled`;
    default:
      return job.error ?? `${verb} failed`;
  }
}

export function candidateModels(
  port: CpsPort | null,
  models: readonly RadioModelDescriptor[],
): RadioModelDescriptor[] {
  if (port === null) {
    return [...models];
  }
  const named = new Set(port.candidate_models);
  return [
    ...models.filter((model) => named.has(model.id)),
    ...models.filter((model) => !named.has(model.id)),
  ];
}

export function modelLabel(model: RadioModelDescriptor): string {
  return `${model.manufacturer} ${model.model}`;
}

export function portOptions(ports: readonly CpsPort[]): { value: string; label: string }[] {
  return ports.map((port) => ({ value: port.port, label: port.label }));
}

export interface IssueGroup {
  severity: ConversionIssue["severity"];
  label: string;
  issues: ConversionIssue[];
}

const SEVERITY_LABELS: Record<ConversionIssue["severity"], string> = {
  dropped: "Left behind",
  adjusted: "Changed to fit",
  note: "Worth knowing",
};

export function groupIssues(report: ConversionReport | null | undefined): IssueGroup[] {
  if (report === null || report === undefined) {
    return [];
  }
  return (["dropped", "adjusted", "note"] as const)
    .map((severity) => ({
      severity,
      label: SEVERITY_LABELS[severity],
      issues: report.issues.filter((issue) => issue.severity === severity),
    }))
    .filter((group) => group.issues.length > 0);
}

export function issueLine(issue: ConversionIssue): string {
  const where = [issue.item, issue.field].filter((part) => part !== undefined).join(" · ");
  return where.length > 0 ? `${where}: ${issue.message}` : issue.message;
}

export function reportSummary(report: ConversionReport | null | undefined): string {
  if (report === null || report === undefined) {
    return "";
  }
  const dropped = report.issues.filter((issue) => issue.severity === "dropped").length;
  const adjusted = report.issues.filter((issue) => issue.severity === "adjusted").length;
  if (dropped === 0 && adjusted === 0) {
    return "Everything fits";
  }
  const parts = [];
  if (dropped > 0) {
    parts.push(`${dropped} left behind`);
  }
  if (adjusted > 0) {
    parts.push(`${adjusted} changed`);
  }
  return parts.join(", ");
}

export function countsLine(report: ConversionReport): string {
  const { before, after } = report;
  const pairs: [string, number, number][] = [
    ["channels", before.channels, after.channels],
    ["contacts", before.contacts, after.contacts],
    ["zones", before.zones, after.zones],
    ["scan lists", before.scan_lists, after.scan_lists],
  ];
  return pairs
    .map(([label, from, to]) => (from === to ? `${to} ${label}` : `${to}/${from} ${label}`))
    .join(" · ");
}
