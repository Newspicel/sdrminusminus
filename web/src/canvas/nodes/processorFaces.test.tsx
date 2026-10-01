import { afterEach, describe, expect, it } from "vitest";
import { useArrayStore } from "../../lib/arrays";
import { useProcessorStore } from "../../lib/processors";
import type { NodeKind, PatchGraph, PatchNode, ProcessorReading } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { arrayStatus, laneStatus, placed } from "../../test/fixtures";
import { CorrelatorFace } from "./CorrelatorFace";
import { PolarimeterFace } from "./PolarimeterFace";
import { SpatialSpectrumFace } from "./SpatialSpectrumFace";
import { StitchFace } from "./StitchFace";

function wired(kind: NodeKind): { node: PatchNode; graph: PatchGraph } {
  const node = placed("proc", catalogBody(kind));
  return {
    node,
    graph: {
      nodes: [placed("arr", catalogBody("array")), node],
      edges: [{ from: { node: "arr", port: "array" }, to: { node: "proc", port: "array" } }],
    },
  };
}

function holding(reading: ProcessorReading): void {
  useProcessorStore.setState({ byNode: { proc: { reading, receivedAt: Date.now() } } });
  useArrayStore
    .getState()
    .seed([arrayStatus("arr", { lanes: [0, 1, 2, 3].map((lane) => laneStatus(lane)) })]);
}

function textOf(html: string): string {
  return html.replace(/<!-- -->/g, "").replace(/<[^>]+>/g, "");
}

afterEach(() => {
  useProcessorStore.getState().reset();
  useArrayStore.getState().reset();
});

describe("array processor faces", () => {
  it("shows the strongest spatial peak and the frame toggle", () => {
    holding({
      type: "spatial_spectrum",
      reading: {
        at: "now",
        azimuth_deg: 90,
        peaks: [{ freq_hz: 433_920_000, bearing_deg: 47, db: -60, true_deg: 137 }],
      },
    });
    const { node, graph } = wired("spatial_spectrum");
    const html = renderFace(SpatialSpectrumFace, node, { graph });
    expect(html).toContain("433.920 MHz 137°");
    expect(html).toContain('aria-label="Bearing frame"');
    expect(html).toContain("4 lanes");
  });

  it("sums up the chosen correlator baseline", () => {
    holding({
      type: "correlator",
      reading: {
        at: "now",
        integrated_s: 10,
        baselines: [{ a: 0, b: 1, delay_ns: 1.234, coherence: 0.87, phase_deg: 37, snr_db: 18 }],
      },
    });
    const { node, graph } = wired("correlator");
    const text = textOf(renderFace(CorrelatorFace, node, { graph }));
    expect(text).toContain("1.23 ns");
    expect(text).toContain("0.87 ∠37° 1.2 ns 18 dB");
    expect(text).toContain("10.0 s");
  });

  it("draws the polarisation ellipse and names the hand", () => {
    holding({
      type: "polarimeter",
      reading: {
        at: "now",
        i_db: -42,
        q: 0.1,
        u: 0.2,
        v: 0.5,
        degree: 0.87,
        angle_deg: 23,
        ellipticity_deg: -5,
        hand: "right",
      },
    });
    const { node, graph } = wired("polarimeter");
    const html = renderFace(PolarimeterFace, node, { graph });
    expect(html).toContain('aria-label="Polarisation"');
    expect(html).toContain("87%");
    expect(html).toContain("RH");
  });

  it("asks to spread an array that tunes lanes together", () => {
    const { node, graph } = wired("stitch");
    const html = renderFace(StitchFace, node, { graph });
    expect(html).toContain(">Needs spread<");
    expect(html).toMatch(/<(?:button)[^>]*>Spread array</);
  });

  it("shows why a processor fails and what it dropped", () => {
    useArrayStore.getState().seed([
      arrayStatus("arr", {
        lanes: [0, 1].map((lane) => laneStatus(lane)),
        processors: [
          {
            node: "proc",
            kind: "polarimeter",
            running: false,
            error: "Too heavy for this band",
            dropped_samples: 12,
            dropped_reports: 0,
            gated_samples: 0,
            lane_mismatch: 0,
            lane_overflows: 0,
            resets: 0,
            solver_failures: 2,
          },
        ],
      }),
    ]);
    const { node, graph } = wired("polarimeter");
    const html = renderFace(PolarimeterFace, node, { graph });
    expect(html).toMatch(/role="alert"[^>]*><p[^>]*>Too heavy for this band</);
    expect(html).toMatch(/>Drops<b[^>]*>12</);
    expect(html).toMatch(/>Fails<b[^>]*>2</);
  });

  it("says no array when nothing is wired", () => {
    const node = placed("proc", catalogBody("correlator"));
    expect(renderFace(CorrelatorFace, node, { graph: { nodes: [node], edges: [] } })).toContain(
      "no array",
    );
  });
});
