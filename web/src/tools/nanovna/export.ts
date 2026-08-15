import type { NanoVnaComplex, NanoVnaSweep } from "../../lib/types";
import { readouts } from "./analysis";
import { gainDb, magnitude, phaseDeg, REFERENCE_OHMS } from "./nanovna";

export type TouchstoneFormat = "ri" | "ma" | "db";

export const TOUCHSTONE_FORMATS: ReadonlyArray<{ value: TouchstoneFormat; label: string }> = [
  { value: "ri", label: "Real / imaginary" },
  { value: "ma", label: "Magnitude / angle" },
  { value: "db", label: "dB / angle" },
];

interface ExportContext {
  recordedAt?: string;
}

/** A two-port Touchstone file. The instrument drives one direction only, so S12 and S22 are
 * written as exact zeros and the header says so — a reader that treats them as measured would
 * otherwise report a perfectly matched, perfectly isolated reverse path. */
export function touchstoneS2p(
  sweep: NanoVnaSweep,
  format: TouchstoneFormat,
  context: ExportContext = {},
): string {
  const zero: NanoVnaComplex = { re: 0, im: 0 };
  const rows = sweep.points.map((point) =>
    [
      String(point.frequency_hz),
      pair(point.s11, format),
      pair(point.s21, format),
      pair(zero, format),
      pair(zero, format),
    ].join(" "),
  );
  return [
    ...header(sweep, context),
    "! S12 and S22 are not measured by this instrument and are written as zero.",
    `# Hz S ${format.toUpperCase()} R ${REFERENCE_OHMS}`,
    "! freq S11 S21 S12 S22",
    ...rows,
    "",
  ].join("\n");
}

/** A one-port Touchstone file: only the reflection the instrument actually measured. */
export function touchstoneS1p(
  sweep: NanoVnaSweep,
  format: TouchstoneFormat,
  context: ExportContext = {},
): string {
  const rows = sweep.points.map((point) =>
    [String(point.frequency_hz), pair(point.s11, format)].join(" "),
  );
  return [
    ...header(sweep, context),
    `# Hz S ${format.toUpperCase()} R ${REFERENCE_OHMS}`,
    "! freq S11",
    ...rows,
    "",
  ].join("\n");
}

const CSV_COLUMNS = [
  "frequency_hz",
  "s11_real",
  "s11_imag",
  "s11_db",
  "s11_phase_deg",
  "vswr",
  "return_loss_db",
  "mismatch_loss_db",
  "resistance_ohm",
  "reactance_ohm",
  "impedance_magnitude_ohm",
  "q",
  "series_capacitance_f",
  "series_inductance_h",
  "conductance_s",
  "susceptance_s",
  "s21_real",
  "s21_imag",
  "s21_db",
  "s21_phase_deg",
  "group_delay_s",
] as const;

/** Every derived quantity the panel shows, one row per measured frequency. */
export function sweepCsv(sweep: NanoVnaSweep): string {
  const rows = readouts(sweep.points).map((row) =>
    [
      row.frequencyHz,
      row.s11.re,
      row.s11.im,
      row.s11Db,
      row.s11PhaseDeg,
      row.vswr,
      row.returnLossDb,
      row.mismatchLossDb,
      row.impedance?.re,
      row.impedance?.im,
      row.impedanceMagnitude,
      row.q,
      row.component?.kind === "capacitance" ? row.component.value : undefined,
      row.component?.kind === "inductance" ? row.component.value : undefined,
      row.admittance?.re,
      row.admittance?.im,
      row.s21.re,
      row.s21.im,
      row.s21Db,
      row.s21PhaseDeg,
      row.groupDelayS,
    ]
      .map(cell)
      .join(","),
  );
  return [CSV_COLUMNS.join(","), ...rows, ""].join("\n");
}

function cell(value: number | undefined): string {
  if (value === undefined || Number.isNaN(value)) {
    return "";
  }
  if (!Number.isFinite(value)) {
    return value > 0 ? "inf" : "-inf";
  }
  return String(value);
}

function header(sweep: NanoVnaSweep, context: ExportContext): string[] {
  const device = sweep.device;
  const calibration = device.calibration.raw === "" ? "none" : device.calibration.raw;
  return [
    "! Measured with sdr-- (https://github.com/sdrminusminus)",
    ...(context.recordedAt === undefined ? [] : [`! Recorded ${context.recordedAt}`]),
    `! Instrument ${device.board ?? "NanoVNA"} firmware ${device.firmware} on ${device.port}`,
    `! Points ${sweep.points.length} averages ${sweep.averages}`,
    ...(device.bandwidth_hz === undefined ? [] : [`! IF bandwidth ${device.bandwidth_hz} Hz`]),
    `! Calibration ${calibration}`,
  ];
}

function pair(value: NanoVnaComplex, format: TouchstoneFormat): string {
  if (format === "ri") {
    return `${fixed(value.re)} ${fixed(value.im)}`;
  }
  if (format === "ma") {
    return `${fixed(magnitude(value))} ${angle(value)}`;
  }
  const db = gainDb(value);
  return `${Number.isFinite(db) ? db.toFixed(6) : "-999.000000"} ${angle(value)}`;
}

function angle(value: NanoVnaComplex): string {
  return phaseDeg(value).toFixed(6);
}

function fixed(value: number): string {
  return Number.isFinite(value) ? value.toFixed(9) : "0.000000000";
}

/** A name that says which instrument took the sweep and over what range, so a folder of them
 * stays readable. */
export function exportFilename(sweep: NanoVnaSweep, extension: string): string {
  const first = sweep.points[0]?.frequency_hz ?? 0;
  const last = sweep.points[sweep.points.length - 1]?.frequency_hz ?? 0;
  const model = (sweep.device.board ?? "nanovna").toLowerCase().replace(/[^a-z0-9]+/g, "-");
  return `${model}-${khz(first)}-to-${khz(last)}.${extension}`;
}

function khz(hz: number): string {
  return `${Math.round(hz / 1000)}khz`;
}

export function downloadText(name: string, mime: string, text: string): void {
  const blob = new Blob([text], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}
