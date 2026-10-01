import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { useProcessorStore } from "../../lib/processors";
import type { NodeBody, PatchGraph } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { placed } from "../../test/fixtures";
import { BeamformerFace } from "./BeamformerFace";
import { NullEditor, STEER_UNWIRED } from "./BeamformerSettings";

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
    expect(renderFace(BeamformerFace, BEAM, { graph: graph(false) })).toContain(STEER_UNWIRED);
    expect(renderFace(BeamformerFace, BEAM, { graph: graph(true) })).not.toContain(STEER_UNWIRED);
  });

  it("lists each null with its own remove button", () => {
    const html = renderFace(BeamformerFace, BEAM, { graph: graph(true) });
    expect(html).toMatch(/>Nulls<\/span><b[^>]*>045°</);
    const settings = BEAM.kind === "beamformer" ? BEAM.data.settings : undefined;
    if (settings === undefined) {
      throw new Error("beamformer settings");
    }
    const editor = renderToStaticMarkup(<NullEditor settings={settings} edit={() => {}} />);
    expect(editor).toContain("045°");
    expect(editor).toContain('aria-label="Remove null 45"');
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
    expect(html).toMatch(/>\+6\.2 <span[^>]*>dB</);
    expect(html).toContain("137° DF");
    expect(html).toContain('title="L2 -1.2 dB 34°"');
    expect(html).toContain('role="meter" aria-label="Lane 2 weight"');
    expect(html).toContain('aria-valuenow="87"');
    expect(html).toContain('aria-label="Beam pattern"');
    expect(html).not.toContain(">No steer<");
  });

  it("puts reading flags in the footer", () => {
    useProcessorStore.setState({
      byNode: {
        beam: {
          reading: {
            type: "beamformer",
            reading: {
              at: "2026-09-28T12:00:00Z",
              output_db: -42,
              nulls_deg: [],
              weights: [],
              pattern: [],
              no_steer: true,
            },
          },
          receivedAt: Date.now(),
        },
      },
    });
    const html = renderFace(BeamformerFace, BEAM, { graph: graph(true) });
    expect(html).toMatch(/title="No bearing to steer at"[^>]*>No steer</);
  });
});
