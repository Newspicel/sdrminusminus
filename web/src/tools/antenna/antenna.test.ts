import { describe, expect, it } from "vitest";
import type { AntennaDesignType, AntennaReport } from "../../lib/types";
import {
  antennaReport,
  antennaRequest,
  DESIGN_OPTIONS,
  defaultDesign,
  formatImpedance,
  formatLength,
  usesFeedline,
} from "./antenna";

describe("defaultDesign", () => {
  it("covers every design the wire union declares", () => {
    for (const option of DESIGN_OPTIONS) {
      expect(defaultDesign(option.value).type).toBe(option.value);
    }
  });

  it("starts the designs that take choices at usable numbers", () => {
    const invertedV = defaultDesign("inverted_v");
    expect(invertedV).toEqual({ type: "inverted_v", settings: { apex_angle_deg: 120 } });

    const yagi = defaultDesign("yagi");
    expect(yagi).toEqual({
      type: "yagi",
      settings: { directors: 2, spacing_wavelengths: 0.2 },
    });

    const groundPlane = defaultDesign("ground_plane");
    expect(groundPlane).toEqual({
      type: "ground_plane",
      settings: { radials: 4, radial_slope_deg: 45 },
    });
  });

  it("sends a design with no choices as a bare tag", () => {
    expect(defaultDesign("dipole")).toEqual({ type: "dipole" });
  });
});

describe("usesFeedline", () => {
  it("asks for the coax factor only where coax is part of the antenna", () => {
    expect(usesFeedline(defaultDesign("quad_loop"))).toBe(true);
    for (const type of ["dipole", "yagi", "j_pole", "end_fed_half_wave"] as AntennaDesignType[]) {
      expect(usesFeedline(defaultDesign(type))).toBe(false);
    }
  });
});

describe("antennaRequest", () => {
  it("tags the body with the tool that answers it", () => {
    const request = antennaRequest({
      frequency_hz: 145_500_000,
      velocity_factor: 0.95,
      feedline_velocity_factor: 0.66,
      design: defaultDesign("dipole"),
    });
    expect(request.tool).toBe("antenna");
    expect(request.request.frequency_hz).toBe(145_500_000);
  });

  it("has nothing to show before an answer arrives", () => {
    expect(antennaReport(undefined)).toBeNull();
  });

  it("unwraps the report out of the tool envelope", () => {
    const report: AntennaReport = {
      design: { type: "dipole" },
      frequency_hz: 145_500_000,
      wavelength_m: 2.06,
      velocity_factor: 0.95,
      parts: [],
      geometry: { segments: [], feed: { x_m: 0, y_m: 0, z_m: 0 } },
      balanced: true,
      notes: [],
    };
    expect(antennaReport({ tool: "antenna", result: report })).toBe(report);
  });
});

describe("formatLength", () => {
  it("drops to centimetres below a metre and keeps millimetre resolution above it", () => {
    expect(formatLength(10.0456, "m")).toBe("10.046 m");
    expect(formatLength(0.482, "m")).toBe("48.2 cm");
  });

  it("carries inches into feet", () => {
    expect(formatLength(10.046, "ft")).toBe("32 ft 11.5 in");
    expect(formatLength(0.2, "ft")).toBe("7.9 in");
  });

  /** A length a hair under a foot boundary must not print as twelve inches. */
  it("rounds before it carries", () => {
    expect(formatLength(12 * 3.99 * 0.0254, "ft")).toBe("3 ft 11.9 in");
    expect(formatLength(4 * 0.3048 - 0.0001, "ft")).toBe("4 ft 0.0 in");
  });

  it("says so when there is no number", () => {
    expect(formatLength(Number.NaN, "m")).toBe("—");
  });
});

describe("formatImpedance", () => {
  it("rounds a real estimate and explains an absent one", () => {
    expect(formatImpedance(73)).toBe("≈ 73 Ω");
    expect(formatImpedance(36.4)).toBe("≈ 36 Ω");
    expect(formatImpedance(null)).toBe("set by its own matching network");
    expect(formatImpedance(undefined)).toBe("set by its own matching network");
  });
});
