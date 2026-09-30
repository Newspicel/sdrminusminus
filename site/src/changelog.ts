import changelog from "../../CHANGELOG.md?raw";

const KINDS = [
  { bump: "major", heading: "Breaking changes" },
  { bump: "minor", heading: "Features" },
  { bump: "patch", heading: "Fixes" },
] as const;

type Bump = (typeof KINDS)[number]["bump"];

export interface Group {
  heading: string;
  items: string[];
}

export interface Release {
  version: string;
  date?: string;
  groups: Group[];
}

interface Pending {
  bump: Bump;
  summary: string;
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function inline(text: string): string {
  return escapeHtml(text)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\[([^\]]+)\]\((https:\/\/[^)\s]+)\)/g, '<a href="$2">$1</a>');
}

export function renderSummary(summary: string): string {
  return summary
    .trim()
    .split(/\n\s*\n/)
    .map((paragraph) => `<p>${inline(paragraph.replace(/\s*\n\s*/g, " "))}</p>`)
    .join("");
}

function parseItems(lines: readonly string[]): string[] {
  const items: string[][] = [];
  for (const line of lines) {
    if (line.startsWith("- ")) {
      items.push([line.slice(2)]);
    } else {
      items.at(-1)?.push(line.replace(/^ {2}/, ""));
    }
  }
  return items.map((item) => renderSummary(item.join("\n")));
}

function parseGroups(lines: readonly string[]): Group[] {
  const groups: { heading: string; lines: string[] }[] = [];
  for (const line of lines) {
    const heading = line.match(/^### (.+)$/)?.[1];
    if (heading !== undefined) {
      groups.push({ heading: heading.trim(), lines: [] });
    } else {
      groups.at(-1)?.lines.push(line);
    }
  }
  return groups
    .map((group) => ({ heading: group.heading, items: parseItems(group.lines) }))
    .filter((group) => group.items.length > 0);
}

export function parseChangelog(text: string): Release[] {
  const sections: { heading: string; lines: string[] }[] = [];
  for (const line of text.replace(/\r\n/g, "\n").split("\n")) {
    const heading = line.match(/^## (.+)$/)?.[1];
    if (heading !== undefined) {
      sections.push({ heading: heading.trim(), lines: [] });
    } else {
      sections.at(-1)?.lines.push(line);
    }
  }
  return sections.map((section) => {
    const [, version = section.heading, date] =
      section.heading.match(/^(\S+)(?: \((\d{4}-\d{2}-\d{2})\))?$/) ?? [];
    return { version, date, groups: parseGroups(section.lines) };
  });
}

export function parseChangeset(text: string): Pending | undefined {
  const [, front = "", body = ""] =
    text.replace(/\r\n/g, "\n").match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/) ?? [];
  const bump = KINDS.find((kind) =>
    new RegExp(`^bump:\\s*"?${kind.bump}"?\\s*$`, "m").test(front),
  )?.bump;
  const summary = body.trim();
  return bump === undefined || summary === "" ? undefined : { bump, summary };
}

export function unreleased(changesets: readonly string[]): Release | undefined {
  const pending = changesets.flatMap((text) => parseChangeset(text) ?? []);
  const groups = KINDS.map((kind) => ({
    heading: kind.heading,
    items: pending
      .filter((change) => change.bump === kind.bump)
      .map((change) => renderSummary(change.summary)),
  })).filter((group) => group.items.length > 0);
  return groups.length === 0 ? undefined : { version: "Next release", groups };
}

export function releases(changelog: string, changesets: readonly string[]): Release[] {
  const next = unreleased(changesets);
  return [...(next === undefined ? [] : [next]), ...parseChangelog(changelog)];
}

export function newest(all: readonly Release[], count: number): Release | undefined {
  const [latest] = all;
  if (latest === undefined) {
    return undefined;
  }
  let left = count;
  const groups = latest.groups.flatMap((group) => {
    const items = group.items.slice(0, left);
    left -= items.length;
    return items.length === 0 ? [] : [{ heading: group.heading, items }];
  });
  return { ...latest, groups };
}

const changesets = import.meta.glob<string>(
  ["../../.changeset/*.md", "!../../.changeset/README.md"],
  { query: "?raw", import: "default", eager: true },
);

export const RELEASES = releases(changelog, Object.values(changesets));
