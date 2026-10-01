import apps from "./data/bench/apps.json";
import decoders from "./data/bench/decoders.json";
import dsp from "./data/bench/dsp.json";

export const SELF = "SDR--";

export interface Entry {
  tool: string;
  version: string;
  value: number;
}

export interface Group {
  id: string;
  title: string;
  unit: string;
  better: "higher" | "lower";
  note?: string;
  results: Entry[];
}

export interface Machine {
  cpu: string;
  os: string;
  date: string;
}

export interface Suite {
  machine: Machine;
  groups: Group[];
}

export interface Section {
  id: string;
  title: string;
  command: string;
  suite: Suite;
}

export const SECTIONS: Section[] = [
  { id: "dsp", title: "DSP kernels", command: "cargo xtask compare dsp", suite: dsp as Suite },
  {
    id: "decoders",
    title: "Decoders",
    command: "cargo xtask compare decoders",
    suite: decoders as Suite,
  },
  { id: "apps", title: "Whole apps", command: "cargo xtask compare apps", suite: apps as Suite },
];

export function ranked(group: Group): Entry[] {
  const sign = group.better === "higher" ? -1 : 1;
  return group.results.toSorted((a, b) => sign * (a.value - b.value));
}

export function width(entry: Entry, group: Group): number {
  const top = Math.max(...group.results.map((result) => result.value));
  return top > 0 ? entry.value / top : 0;
}

export function amount(value: number): string {
  if (value >= 100 || Number.isInteger(value)) return Math.round(value).toLocaleString("en-US");
  if (value >= 10) return value.toFixed(1);
  return value.toFixed(2);
}

export function machine(suite: Suite): string {
  return [suite.machine.cpu, suite.machine.os, suite.machine.date].filter(Boolean).join(" · ");
}

export function measured(sections: readonly Section[]): Section[] {
  return sections.filter((section) => section.suite.groups.length > 0);
}
