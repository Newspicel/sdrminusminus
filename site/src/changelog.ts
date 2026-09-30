import { REPOSITORY } from "./seo";

const HEADINGS = ["Breaking changes", "Features", "Fixes"];

export interface Group {
  heading: string;
  items: string[];
}

export interface Release {
  version: string;
  date: string;
  groups: Group[];
}

export interface GitHubRelease {
  tag_name: string;
  published_at: string | null;
  draft: boolean;
  prerelease: boolean;
  body: string | null;
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
  let open = false;
  for (const line of lines) {
    if (line.startsWith("- ")) {
      items.push([line.slice(2)]);
      open = true;
    } else if (open && (line.trim() === "" || line.startsWith("  "))) {
      items.at(-1)?.push(line.replace(/^ {2}/, ""));
    } else {
      open = false;
    }
  }
  return items.map((item) => renderSummary(item.join("\n")));
}

export function parseNotes(body: string): Group[] {
  const groups: { heading: string; lines: string[] }[] = [];
  let current: { heading: string; lines: string[] } | undefined;
  for (const line of body.replace(/\r\n/g, "\n").split("\n")) {
    const heading = line.match(/^#{1,6} (.+)$/)?.[1]?.trim();
    if (heading !== undefined) {
      current = HEADINGS.includes(heading) ? { heading, lines: [] } : undefined;
      if (current !== undefined) {
        groups.push(current);
      }
    } else {
      current?.lines.push(line);
    }
  }
  return groups
    .map((group) => ({ heading: group.heading, items: parseItems(group.lines) }))
    .filter((group) => group.items.length > 0);
}

export function fromGitHub(all: readonly GitHubRelease[]): Release[] {
  return all.flatMap((release) => {
    const version = release.tag_name.match(/^v(\d+\.\d+\.\d+)$/)?.[1];
    if (version === undefined || release.draft || release.prerelease || !release.published_at) {
      return [];
    }
    const groups = parseNotes(release.body ?? "");
    return groups.length === 0
      ? []
      : [{ version, date: release.published_at.slice(0, 10), groups }];
  });
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

const API = REPOSITORY.replace("https://github.com/", "https://api.github.com/repos/");

async function fetchReleases(): Promise<Release[]> {
  const token: string | undefined = import.meta.env.GITHUB_TOKEN;
  const response = await fetch(`${API}/releases?per_page=30`, {
    headers: {
      accept: "application/vnd.github+json",
      "user-agent": "sdrmm-site",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub releases: ${response.status} ${response.statusText}`);
  }
  return fromGitHub((await response.json()) as GitHubRelease[]);
}

let cached: Promise<Release[]> | undefined;

export function releases(): Promise<Release[]> {
  cached ??= fetchReleases();
  return cached;
}
