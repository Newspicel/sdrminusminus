import type { PointReadout } from "./analysis";
import { formatSi } from "./nanovna";

export type ChartId = "magnitude" | "vswr" | "phase" | "impedance" | "delay" | "smith";

export interface Series {
  key: string;
  label: string;
  stroke: string;
  valueOf: (row: PointReadout) => number;
}

export interface Domain {
  low: number;
  high: number;
}

export interface ChartView {
  id: ChartId;
  label: string;
  unit: string;
  series: Series[];
  domain: (values: number[]) => Domain;
  format: (value: number) => string;
}

const TRACE = "var(--color-plot-trace)";
const SECOND = "var(--color-plot-hold)";

const MAGNITUDE_FLOOR_DB = 60;

export const CHART_VIEWS: readonly ChartView[] = [
  {
    id: "magnitude",
    label: "Magnitude",
    unit: "dB",
    series: [
      { key: "s11", label: "S11", stroke: TRACE, valueOf: (row) => row.s11Db },
      { key: "s21", label: "S21", stroke: SECOND, valueOf: (row) => row.s21Db },
    ],
    domain: (values) => floored(snapped(values, 10, 20), MAGNITUDE_FLOOR_DB),
    format: (value) => value.toFixed(0),
  },
  {
    id: "vswr",
    label: "VSWR",
    unit: ":1",
    series: [{ key: "vswr", label: "VSWR", stroke: TRACE, valueOf: (row) => row.vswr }],
    domain: (values) => ({
      low: 1,
      high: Math.min(20, Math.max(2, Math.ceil(finiteMax(values, 3)))),
    }),
    format: (value) => value.toFixed(1),
  },
  {
    id: "phase",
    label: "Phase",
    unit: "°",
    series: [
      { key: "s11", label: "S11", stroke: TRACE, valueOf: (row) => row.s11PhaseDeg },
      { key: "s21", label: "S21", stroke: SECOND, valueOf: (row) => row.s21PhaseDeg },
    ],
    domain: () => ({ low: -180, high: 180 }),
    format: (value) => value.toFixed(0),
  },
  {
    id: "impedance",
    label: "Impedance",
    unit: "Ω",
    series: [
      { key: "r", label: "R", stroke: TRACE, valueOf: (row) => row.impedance?.re ?? Number.NaN },
      { key: "x", label: "X", stroke: SECOND, valueOf: (row) => row.impedance?.im ?? Number.NaN },
    ],
    domain: (values) => snapped([...values, 0], 25, 50),
    format: (value) => value.toFixed(0),
  },
  {
    id: "delay",
    label: "Group delay",
    unit: "s",
    series: [
      { key: "delay", label: "S21 delay", stroke: TRACE, valueOf: (row) => row.groupDelayS },
    ],
    domain: (values) => padded(values),
    format: (value) => formatSi(value, "s", 1),
  },
  {
    id: "smith",
    label: "Smith",
    unit: "",
    series: [{ key: "s11", label: "S11", stroke: TRACE, valueOf: (row) => row.s11Linear }],
    domain: () => ({ low: -1, high: 1 }),
    format: (value) => value.toFixed(2),
  },
];

export function chartView(id: ChartId): ChartView {
  return CHART_VIEWS.find((view) => view.id === id) ?? (CHART_VIEWS[0] as ChartView);
}

export function seriesValues(view: ChartView, rows: readonly PointReadout[]): number[] {
  return view.series.flatMap((series) => rows.map((row) => series.valueOf(row)));
}

function snapped(values: number[], step: number, minimumSpan: number): Domain {
  const finite = values.filter(Number.isFinite);
  if (finite.length === 0) {
    return { low: -step, high: step };
  }
  const low = Math.floor(Math.min(...finite) / step) * step;
  const high = Math.ceil(Math.max(...finite) / step) * step;
  if (high - low >= minimumSpan) {
    return { low, high };
  }
  const middle = (low + high) / 2;
  return { low: middle - minimumSpan / 2, high: middle + minimumSpan / 2 };
}

function floored(domain: Domain, depth: number): Domain {
  return { low: Math.max(domain.low, domain.high - depth), high: domain.high };
}

function padded(values: number[]): Domain {
  const finite = values.filter(Number.isFinite);
  if (finite.length === 0) {
    return { low: -1, high: 1 };
  }
  const low = Math.min(...finite);
  const high = Math.max(...finite);
  const margin = (high - low) * 0.1 || Math.abs(high) * 0.1 || 1;
  return { low: low - margin, high: high + margin };
}

function finiteMax(values: number[], fallback: number): number {
  const finite = values.filter(Number.isFinite);
  return finite.length === 0 ? fallback : Math.max(...finite);
}
