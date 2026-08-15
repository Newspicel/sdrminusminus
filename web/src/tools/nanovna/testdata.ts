import type {
  NanoVnaComplex,
  NanoVnaDeviceReport,
  NanoVnaPoint,
  NanoVnaSweep,
} from "../../lib/types";

export const DEVICE: NanoVnaDeviceReport = {
  port: "/dev/cu.usbmodem4001",
  firmware: "1.2.46",
  board: "NanoVNA-H 4",
  info: ["Board: NanoVNA-H 4", "Platform: STM32F303xC Analog & DSP"],
  battery_mv: 4177,
  bandwidth_hz: 1000,
  power: 255,
  tcxo_hz: 26_000_000,
  harmonic_threshold_hz: 300_000_100,
  electrical_delay_s: 0,
  s21_offset_db: 0,
  sweep: { start_hz: 50_000, stop_hz: 900_000_000, points: 101 },
  calibration: {
    port: "/dev/cu.usbmodem4001",
    standards: ["load", "isolation"],
    error_terms: ["Es", "Er", "Et"],
    applied: true,
    raw: "load isoln Es Er Et cal'ed",
  },
  commands: ["scan", "data", "frequencies", "sweep", "cal"],
};

export function point(
  frequencyHz: number,
  s11: NanoVnaComplex,
  s21: NanoVnaComplex = { re: 0, im: 0 },
): NanoVnaPoint {
  return { frequency_hz: frequencyHz, s11, s21 };
}

export function sweepOf(
  points: NanoVnaPoint[],
  overrides: Partial<NanoVnaSweep> = {},
): NanoVnaSweep {
  return {
    device: DEVICE,
    requested_points: points.length,
    averages: 1,
    elapsed_ms: 1234,
    points,
    ...overrides,
  };
}

export function resonantSweep(count = 201): NanoVnaSweep {
  const centreHz = 14_100_000;
  const resistance = 50;
  const inductance = 1e-5;
  const capacitance = 1 / (inductance * (2 * Math.PI * centreHz) ** 2);
  const points = Array.from({ length: count }, (_, index) => {
    const frequencyHz = 13_000_000 + (index / (count - 1)) * 2_000_000;
    const omega = 2 * Math.PI * frequencyHz;
    const reactance = omega * inductance - 1 / (omega * capacitance);
    return point(Math.round(frequencyHz), reflection(resistance, reactance));
  });
  return sweepOf(points);
}

function reflection(resistance: number, reactance: number, reference = 50): NanoVnaComplex {
  const numeratorRe = resistance - reference;
  const denominatorRe = resistance + reference;
  const denominator = denominatorRe * denominatorRe + reactance * reactance;
  return {
    re: (numeratorRe * denominatorRe + reactance * reactance) / denominator,
    im: (reactance * denominatorRe - numeratorRe * reactance) / denominator,
  };
}
