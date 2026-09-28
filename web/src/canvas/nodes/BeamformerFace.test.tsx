import { afterEach, describe, expect, it } from "vitest";
import { useProcessorStore } from "../../lib/processors";
import type { NodeBody, PatchGraph } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { placed } from "../../test/fixtures";
import { BeamformerFace } from "./BeamformerFace";

function lcmv(): NodeBody {
  const body = catalogBody("beamformer");
  if (body.kind !== "beamformer" || body.data.settings === undefined) {
    throw new Error("a beamformer body");
  }
  return { ...body, data: { settings: { ...body.data.settings, mode: "lcmv", nulls_deg: [45] } } };
}

const BEAM = placed("beam", lcmv());

function graph(steer: boolean): PatchGraph {
  return {
    nodes: [placed("arr", catalogBody("array")), placed("finder", catalogBody("df")), BEAM],
    edges: [
      { from: { node: "arr", port: "array" }, to: { node: "beam", port: "array" } },
      ...(steer
        ? [{ from: { node: "finder", port: "events" }, to: { node: "beam", port: "steer" } }]
        : []),
    ],
  };
}

afterEach(() => useProcessorStore.getState().reset());

describe("BeamformerFace", () => {
  it("disables DF steering without a steer wire", () => {
    expect(renderFace(BeamformerFace, BEAM, { graph: graph(false) })).toContain(
      "Wire a DF to steer",
    );
    expect(renderFace(BeamformerFace, BEAM, { graph: graph(true) })).not.toContain(
      "Wire a DF to steer",
    );
  });

  it("lists each null with its own remove button", () => {
    const html = renderFace(BeamformerFace, BEAM, { graph: graph(true) });
    expect(html).toContain("045°");
    expect(html).toContain('aria-label="Remove null 45"');
  });

  it("shows gain, steer and weights from a reading", () => {
    useProcessorStore.setState({
      byNode: {
        beam: {
          reading: {
            type: "beamformer",
            reading: {
              at: "2026-09-28T12:00:00Z",
              output_db: -42,
              sinr_gain_db: 6.2,
              steer_deg: 137,
              nulls_deg: [45],
              weights: [
                { amplitude_db: 0, phase_deg: 0 },
                { amplitude_db: -1.2, phase_deg: 34 },
              ],
              pattern: Array.from({ length: 360 }, () => 128),
            },
          },
          receivedAt: Date.now(),
        },
      },
    });
    const html = renderFace(BeamformerFace, BEAM, { graph: graph(true) });
    expect(html).toContain("+6.2 dB");
    expect(html).toContain("137° DF");
    expect(html).toContain('title="L2 -1.2 dB 34°"');
    expect(html).toContain('aria-label="Beam pattern"');
  });
});
