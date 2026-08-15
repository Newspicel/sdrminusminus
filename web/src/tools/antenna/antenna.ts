import type { Options } from "../../components/controls";
import type {
  AntennaDesign,
  AntennaDesignType,
  AntennaReport,
  AntennaRequest,
  ToolRequest,
  ToolResponse,
} from "../../lib/types";

export const DESIGN_OPTIONS: Options<AntennaDesignType> = [
  { value: "dipole", label: "Dipole (half wave)" },
  { value: "inverted_v", label: "Inverted V" },
  { value: "ground_plane", label: "Ground plane (quarter wave)" },
  { value: "five_eighths_vertical", label: "5/8 wave vertical" },
  { value: "folded_dipole", label: "Folded dipole" },
  { value: "j_pole", label: "J-pole" },
  { value: "yagi", label: "Yagi" },
  { value: "quad_loop", label: "Quad loop (full wave)" },
  { value: "end_fed_half_wave", label: "End-fed half wave" },
];

export function designLabel(type: AntennaDesignType): string {
  return DESIGN_OPTIONS.find((option) => option.value === type)?.label ?? type;
}

/** What the panel starts a design at. The server fills in the same numbers for a design sent
 * without settings; these exist so the controls have something to show before the first
 * answer comes back. */
export function defaultDesign(type: AntennaDesignType): AntennaDesign {
  switch (type) {
    case "inverted_v":
      return { type, settings: { apex_angle_deg: 120 } };
    case "ground_plane":
      return { type, settings: { radials: 4, radial_slope_deg: 45 } };
    case "yagi":
      return { type, settings: { directors: 2, spacing_wavelengths: 0.2 } };
    default:
      return { type };
  }
}

/** Whether the coax velocity factor is part of this design — only the ones that include a
 * matching section made of coax read it. */
export function usesFeedline(design: AntennaDesign): boolean {
  return design.type === "quad_loop";
}

export function antennaRequest(request: AntennaRequest): ToolRequest {
  return { tool: "antenna", request };
}

export function antennaReport(response: ToolResponse | undefined): AntennaReport | null {
  return response === undefined ? null : response.result;
}

export type LengthUnit = "m" | "ft";

export const UNIT_OPTIONS: Options<LengthUnit> = [
  { value: "m", label: "m" },
  { value: "ft", label: "ft" },
];

const INCHES_PER_M = 39.370_078_7;

/** A length as something to measure with: metric below a metre goes to centimetres, imperial
 * carries into feet and inches. */
export function formatLength(meters: number, unit: LengthUnit): string {
  if (!Number.isFinite(meters)) {
    return "—";
  }
  if (unit === "ft") {
    // Rounded before the carry, or a length a hair under a foot boundary prints as "12.0 in".
    const inches = Math.round(meters * INCHES_PER_M * 10) / 10;
    const feet = Math.floor(inches / 12);
    return feet > 0
      ? `${feet} ft ${(inches - feet * 12).toFixed(1)} in`
      : `${inches.toFixed(1)} in`;
  }
  return meters < 1 ? `${(meters * 100).toFixed(1)} cm` : `${meters.toFixed(3)} m`;
}

export function formatImpedance(ohms: number | null | undefined): string {
  return ohms == null ? "set by its own matching network" : `≈ ${Math.round(ohms)} Ω`;
}
