import { describe, expect, it } from "vitest";
import type { ClientEvent } from "../lib/diagnostics";
import type { DiagnosticsReport, PatchGraph } from "../lib/types";
import {
  bugIssueUrl,
  buildBundle,
  featureIssueUrl,
  issueTitle,
  MAX_BUNDLE_LOG_LINES,
  MAX_ISSUE_URL,
  PASTE_MARKER,
  workspaceFacts,
} from "./report";

const REPO = "https://github.com/Newspicel/sdrminusminus";

function diagnostics(overrides: Partial<DiagnosticsReport> = {}): DiagnosticsReport {
  return {
    generated_at: "2026-09-16T10:00:00Z",
    doctor: {
      version: "0.4.0",
      platform: "macos/aarch64",
      checks: [
        {
          id: "backends",
          name: "Device backends",
          status: "warn",
          detail: "compiled backends: virtual",
          hint: "rebuild with --features soapy",
        },
        { id: "storage.db", name: "Database", status: "ok", detail: "~/Library/sdrmm.db" },
      ],
    },
    log: [
      {
        at: "2026-09-16T09:59:00Z",
        level: "warn",
        target: "sdrmm_engine",
        message: "device open failed",
      },
    ],
    dropped: 0,
    ...overrides,
  };
}

function events(): ClientEvent[] {
  return [
    { at: "2026-09-16T09:59:30Z", level: "error", source: "toast", message: "[engine] boom" },
  ];
}

describe("workspaceFacts", () => {
  it("counts kinds without carrying names, positions or frequencies", () => {
    const graph = {
      nodes: [
        { id: "device-1", kind: "device", position: { x: 1, y: 2 }, data: { label: "Home RTL" } },
        { id: "channel-1", kind: "channel", position: { x: 3, y: 4 }, data: { label: "145.5" } },
        { id: "channel-2", kind: "channel", position: { x: 5, y: 6 }, data: {} },
      ],
      edges: [{ id: "e1", source: "device-1", target: "channel-1" }],
    } as unknown as PatchGraph;

    const facts = workspaceFacts(graph);
    expect(facts).toEqual({
      nodes: 3,
      edges: 1,
      kinds: [
        { kind: "channel", count: 2 },
        { kind: "device", count: 1 },
      ],
    });
    expect(JSON.stringify(facts)).not.toContain("Home RTL");
    expect(JSON.stringify(facts)).not.toContain("145.5");
  });
});

describe("buildBundle", () => {
  it("carries the environment, the checks and both logs", () => {
    const bundle = buildBundle({
      version: "0.4.0",
      userAgent: "Mozilla/5.0 Test",
      diagnostics: diagnostics(),
      events: events(),
      droppedEvents: 0,
      workspace: { nodes: 3, edges: 1, kinds: [{ kind: "device", count: 1 }] },
    });

    expect(bundle).toContain("| SDR-- | 0.4.0 |");
    expect(bundle).toContain("| Platform | macos/aarch64 |");
    expect(bundle).toContain("**WARN** Device backends");
    expect(bundle).toContain("hint: rebuild with --features soapy");
    expect(bundle).toContain("3 nodes, 1 edges: device ×1");
    expect(bundle).toContain("device open failed");
    expect(bundle).toContain("[engine] boom");
  });

  it("leaves out the workspace when it is not offered", () => {
    const bundle = buildBundle({
      version: "0.4.0",
      userAgent: "Test",
      diagnostics: diagnostics(),
      events: [],
      droppedEvents: 0,
      workspace: null,
    });
    expect(bundle).not.toContain("### Workspace");
    expect(bundle).not.toContain("### Client log");
  });

  it("still reports when the server could not be reached", () => {
    const bundle = buildBundle({
      version: "0.4.0",
      userAgent: "Test",
      diagnostics: null,
      events: events(),
      droppedEvents: 2,
      workspace: null,
    });
    expect(bundle).toContain("| Platform | unknown |");
    expect(bundle).not.toContain("### Diagnostics");
    expect(bundle).toContain("### Client log (1 lines, 2 older dropped)");
  });

  it("caps the server log and says what it left out", () => {
    const log = Array.from({ length: MAX_BUNDLE_LOG_LINES + 40 }, (_unused, index) => ({
      at: "2026-09-16T09:59:00Z",
      level: "info" as const,
      target: "sdrmm",
      message: `line ${index}`,
    }));
    const bundle = buildBundle({
      version: "0.4.0",
      userAgent: "Test",
      diagnostics: diagnostics({ log, dropped: 7 }),
      events: [],
      droppedEvents: 0,
      workspace: null,
    });

    expect(bundle).toContain(`### Server log (${MAX_BUNDLE_LOG_LINES} lines, 47 older dropped)`);
    expect(bundle).not.toContain("line 39\n");
    expect(bundle).toContain(`line ${MAX_BUNDLE_LOG_LINES + 39}`);
  });
});

describe("issue urls", () => {
  it("prefills the bug form with the bundle when it fits", () => {
    const url = bugIssueUrl(REPO, "Device open failed", "0.4.0", "short bundle");
    expect(url.startsWith(`${REPO}/issues/new?template=bug.yml&`)).toBe(true);
    expect(url).toContain("title=Device%20open%20failed");
    expect(url).toContain("version=0.4.0");
    expect(url).toContain(`environment=${encodeURIComponent("short bundle")}`);
  });

  it("falls back to a paste marker rather than a url GitHub would reject", () => {
    const url = bugIssueUrl(REPO, "Big one", "0.4.0", "x".repeat(20_000));
    expect(url.length).toBeLessThanOrEqual(MAX_ISSUE_URL);
    expect(url).toContain(encodeURIComponent(PASTE_MARKER));
  });

  it("tolerates a repository url with a trailing slash", () => {
    expect(featureIssueUrl(`${REPO}/`, "0.4.0")).toBe(
      `${REPO}/issues/new?template=feature.yml&version=0.4.0`,
    );
  });

  it("drops an empty parameter instead of sending a blank field", () => {
    expect(featureIssueUrl(REPO, "")).toBe(`${REPO}/issues/new?template=feature.yml`);
  });
});

describe("issueTitle", () => {
  it("collapses whitespace and caps the length", () => {
    expect(issueTitle("  device   open\nfailed ")).toBe("device open failed");
    expect(issueTitle(null)).toBe("");
    expect(issueTitle("y".repeat(200))).toHaveLength(120);
  });
});
