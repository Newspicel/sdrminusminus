import type { ClientEvent } from "../lib/diagnostics";
import type { DiagnosticsReport, PatchGraph } from "../lib/types";

export const MAX_ISSUE_URL = 6000;
export const MAX_BUNDLE_LOG_LINES = 200;

export const PASTE_MARKER = "Paste the diagnostics bundle here — it is already on your clipboard.";

export interface WorkspaceFacts {
  nodes: number;
  edges: number;
  kinds: readonly { kind: string; count: number }[];
}

export interface BundleInput {
  version: string;
  userAgent: string;
  diagnostics: DiagnosticsReport | null;
  events: readonly ClientEvent[];
  droppedEvents: number;
  workspace: WorkspaceFacts | null;
}

export function workspaceFacts(graph: PatchGraph): WorkspaceFacts {
  const counts = new Map<string, number>();
  const edges = graph.edges ?? [];
  for (const node of graph.nodes) {
    counts.set(node.kind, (counts.get(node.kind) ?? 0) + 1);
  }
  return {
    nodes: graph.nodes.length,
    edges: edges.length,
    kinds: [...counts.entries()]
      .map(([kind, count]) => ({ kind, count }))
      .toSorted((a, b) => b.count - a.count || a.kind.localeCompare(b.kind)),
  };
}

export function buildBundle(input: BundleInput): string {
  const sections = [
    environmentSection(input),
    doctorSection(input.diagnostics),
    workspaceSection(input.workspace),
    serverLogSection(input.diagnostics),
    clientLogSection(input.events, input.droppedEvents),
  ];
  return sections.filter((section) => section !== null).join("\n\n");
}

function environmentSection(input: BundleInput): string {
  const doctor = input.diagnostics?.doctor;
  const rows: [string, string][] = [
    ["SDR--", input.version || "unknown"],
    ["Platform", doctor?.platform ?? "unknown"],
    ["Browser", input.userAgent || "unknown"],
    ["Collected", input.diagnostics?.generated_at ?? new Date().toISOString()],
  ];
  const body = rows.map(([name, value]) => `| ${name} | ${value} |`).join("\n");
  return `### Environment\n\n| | |\n| --- | --- |\n${body}`;
}

function doctorSection(diagnostics: DiagnosticsReport | null): string | null {
  const checks = diagnostics?.doctor.checks ?? [];
  if (checks.length === 0) {
    return null;
  }
  const lines = checks.map((check) => {
    const hint = check.hint === undefined || check.hint === null ? "" : `\n  hint: ${check.hint}`;
    return `- **${check.status.toUpperCase()}** ${check.name} — ${check.detail}${hint}`;
  });
  return `### Diagnostics\n\n${lines.join("\n")}`;
}

function workspaceSection(workspace: WorkspaceFacts | null): string | null {
  if (workspace === null) {
    return null;
  }
  const kinds = workspace.kinds.map((entry) => `${entry.kind} ×${entry.count}`).join(", ");
  return `### Workspace\n\n${workspace.nodes} nodes, ${workspace.edges} edges${
    kinds.length > 0 ? `: ${kinds}` : ""
  }`;
}

function serverLogSection(diagnostics: DiagnosticsReport | null): string | null {
  const log = diagnostics?.log ?? [];
  if (log.length === 0) {
    return null;
  }
  const shown = log.slice(-MAX_BUNDLE_LOG_LINES);
  const omitted = (diagnostics?.dropped ?? 0) + (log.length - shown.length);
  const lines = shown.map(
    (line) => `${line.at} ${line.level.toUpperCase()} ${line.target}: ${line.message}`,
  );
  return `### Server log${countNote(shown.length, omitted)}\n\n\`\`\`\n${lines.join("\n")}\n\`\`\``;
}

function clientLogSection(events: readonly ClientEvent[], dropped: number): string | null {
  if (events.length === 0) {
    return null;
  }
  const lines = events.map(
    (event) => `${event.at} ${event.level.toUpperCase()} ${event.source}: ${event.message}`,
  );
  return `### Client log${countNote(events.length, dropped)}\n\n\`\`\`\n${lines.join("\n")}\n\`\`\``;
}

function countNote(shown: number, omitted: number): string {
  const suffix = omitted > 0 ? `, ${omitted} older dropped` : "";
  return ` (${shown} lines${suffix})`;
}

export function bugIssueUrl(
  repository: string,
  title: string,
  version: string,
  bundle: string,
): string {
  const base = `${issuesBase(repository)}?template=bug.yml`;
  const full = withParams(base, { title, version, environment: bundle });
  if (full.length <= MAX_ISSUE_URL) {
    return full;
  }
  return withParams(base, { title, version, environment: PASTE_MARKER });
}

export function featureIssueUrl(repository: string, version: string): string {
  return withParams(`${issuesBase(repository)}?template=feature.yml`, { version });
}

function issuesBase(repository: string): string {
  return `${repository.replace(/\/+$/, "")}/issues/new`;
}

function withParams(base: string, params: Record<string, string>): string {
  const query = Object.entries(params)
    .filter(([, value]) => value.length > 0)
    .map(([name, value]) => `${name}=${encodeURIComponent(value)}`)
    .join("&");
  return query.length > 0 ? `${base}&${query}` : base;
}

export function issueTitle(seed: string | null): string {
  const trimmed = (seed ?? "").trim().replace(/\s+/g, " ");
  if (trimmed.length === 0) {
    return "";
  }
  return trimmed.length <= 120 ? trimmed : `${trimmed.slice(0, 119)}…`;
}
