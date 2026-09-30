import { afterEach, describe, expect, it } from "vitest";
import { useArrayStore } from "../../lib/arrays";
import { useProcessorStore } from "../../lib/processors";
import type {
  ArrayStatus,
  DfPeak,
  DfReading,
  PatchGraph,
  PatchNode,
  ProcessorStatus,
} from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { arrayStatus, laneStatus, placed } from "../../test/fixtures";
import { DfFace } from "./DfFace";
import { NEEDS_HEADING } from "./df";

const FINDER: PatchNode = placed("finder", catalogBody("df"));

const WIRED: PatchGraph = {
  nodes: [placed("north", catalogBody("array")), FINDER],
  edges: [{ from: { node: "north", port: "array" }, to: { node: "finder", port: "array" } }],
};

const PEAK: DfPeak = {
  relative_deg: 47,
  true_deg: 137,
  power_db: -40,
  confidence: 0.82,
  sigma_deg: 3.1,
};

function reading(overrides: Partial<DfReading> = {}): DfReading {
  return {
    at: "2026-09-28T12:00:00Z",
    peaks: [PEAK],
    pseudospectrum: Array.from({ length: 360 }, (_, deg) => (deg === 47 ? 255 : 20)),
    azimuth_deg: 90,
    station: { lat: 52, lon: 13 },
    sources: 1,
    sources_auto: true,
    squelched: false,
    aliasing: false,
    fit: 0.94,
    snr_db: 18.2,
    ...overrides,
  };
}

function gated(
  gate: ProcessorStatus["gated"],
  overrides: Partial<ProcessorStatus> = {},
): ProcessorStatus {
  return {
    node: "finder",
    kind: "df",
    running: true,
    gated: gate,
    gated_samples: 0,
    dropped_samples: 0,
    dropped_reports: 0,
    lane_overflows: 0,
    lane_mismatch: 0,
    solver_failures: 0,
    resets: 0,
    ...overrides,
  };
}

function traceStart(html: string): { x: number; y: number } {
  const match = /<path d="M([\d.]+) ([\d.]+)[^"]*" class="fill-accent\/15/.exec(html);
  return { x: Number(match?.[1]), y: Number(match?.[2]) };
}

const FIVE: ArrayStatus = arrayStatus("north", {
  lanes: [0, 1, 2, 3, 4].map((lane) => laneStatus(lane)),
});

function render(found: DfReading | null, status: ArrayStatus = FIVE, graph = WIRED): string {
  useArrayStore.setState({ byNode: { north: status }, receivedAt: { north: Date.now() } });
  useProcessorStore.setState({
    byNode:
      found === null
        ? {}
        : { finder: { reading: { type: "df", reading: found }, receivedAt: Date.now() } },
  });
  return renderFace(DfFace, FINDER, { graph });
}

function attributesOf(html: string, text: string): string {
  return new RegExp(`<(\\w+)([^<>]*)>${text}</\\1>`).exec(html)?.[2] ?? "";
}

function pressed(html: string, label: string): boolean {
  return attributesOf(html, label).includes('aria-pressed="true"');
}

afterEach(() => {
  useArrayStore.getState().reset();
  useProcessorStore.getState().reset();
});

describe("DfFace", () => {
  it("shows the bearing in the true frame when heading is known", () => {
    const html = render(reading());
    expect(pressed(html, "True")).toBe(true);
    expect(html).toMatch(/>Bearing<.*>137°</);
    expect(html).toMatch(/>Fit<.*>94%</);
    expect(html).toContain("5 lanes");
    expect(html).toContain('aria-label="Bearing rose"');
    expect(html).toContain(">N</text>");
    expect(html).toContain('aria-label="Method"');
  });

  it("disables True without heading", () => {
    const html = render(reading({ azimuth_deg: null, peaks: [{ ...PEAK, true_deg: null }] }));
    expect(pressed(html, "Rel")).toBe(true);
    expect(html).toContain(`title="${NEEDS_HEADING}"`);
    expect(attributesOf(html, "True")).toContain('disabled=""');
    expect(html).toMatch(/>Bearing<.*>047°</);
    expect(html).toContain(">F</text>");
    expect(html).toContain(">No heading<");
  });

  it("explains a gated finder", () => {
    const html = render(null, { ...FIVE, processors: [gated("phase")] });
    expect(html).toContain("needs cal");
    expect(html).toContain(">Needs cal<");
    expect(html).toMatch(/>Bearing<.*>-</);
  });

  it("says when no array is wired", () => {
    const html = render(null, FIVE, { nodes: WIRED.nodes, edges: [] });
    expect(html).toContain("no array");
  });

  it("turns the trace into the true frame", () => {
    const centre = 110;
    const turned = traceStart(render(reading()));
    expect(turned.x).toBeGreaterThan(centre);
    expect(turned.y).toBeCloseTo(centre, 1);
    const relative = traceStart(render(reading({ azimuth_deg: null })));
    expect(relative.x).toBeCloseTo(centre, 1);
    expect(relative.y).toBeLessThan(centre);
  });

  it("shows what the processor lost and why it stopped", () => {
    const failing = gated(undefined, {
      dropped_reports: 3,
      truncated: 2,
      error: "Needs a line or circle",
    });
    const html = render(reading(), { ...FIVE, processors: [failing] });
    expect(html).toMatch(/>Lost<.*>3</);
    expect(html).toMatch(/>Cut<.*>2</);
    expect(html).not.toContain(">Drops<");
    expect(html).toMatch(/>Fault<.*>Needs a line or circle</);
  });
});
