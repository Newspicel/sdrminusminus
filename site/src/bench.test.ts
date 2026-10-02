import { describe, expect, it } from "vitest";
import {
  amount,
  type Group,
  highlights,
  lead,
  machine,
  measured,
  ours,
  ranked,
  SECTIONS,
  SELF,
  type Section,
  width,
} from "./bench";

const group = (better: Group["better"]): Group => ({
  id: "fir",
  title: "FIR",
  unit: "Msps",
  better,
  results: [
    { tool: "a", version: "1", value: 50 },
    { tool: SELF, version: "1", value: 200 },
    { tool: "b", version: "1", value: 100 },
  ],
});

describe("ranked", () => {
  it("puts the best result first", () => {
    expect(ranked(group("higher")).map((entry) => entry.value)).toEqual([200, 100, 50]);
    expect(ranked(group("lower")).map((entry) => entry.value)).toEqual([50, 100, 200]);
  });
});

describe("width", () => {
  it("scales bars to the largest value", () => {
    const fir = group("higher");
    expect(fir.results.map((entry) => width(entry, fir))).toEqual([0.25, 1, 0.5]);
  });
});

describe("amount", () => {
  it("keeps three significant figures", () => {
    expect(amount(1234.5)).toBe("1,235");
    expect(amount(12.34)).toBe("12.3");
    expect(amount(1.234)).toBe("1.23");
  });

  it("shows counts without decimals", () => {
    expect(amount(6)).toBe("6");
    expect(amount(21)).toBe("21");
  });
});

describe("machine", () => {
  it("skips missing parts", () => {
    expect(machine({ machine: { cpu: "M4", os: "", date: "2026-10-01" }, groups: [] })).toBe(
      "M4 · 2026-10-01",
    );
  });
});

describe("lead", () => {
  it("compares us with the best other tool", () => {
    expect(lead(group("higher"))).toBe(2);
    expect(lead(group("lower"))).toBe(0.25);
  });

  it("uses our weakest variant", () => {
    const results = [
      { tool: `${SELF} app`, version: "1", value: 40 },
      { tool: `${SELF} headless`, version: "1", value: 10 },
      { tool: "other", version: "1", value: 20 },
    ];
    expect(lead({ ...group("lower"), results })).toBe(0.5);
  });

  it("is zero without a rival", () => {
    expect(lead({ ...group("higher"), results: [{ tool: SELF, version: "1", value: 1 }] })).toBe(0);
  });
});

describe("highlights", () => {
  const section = (groups: Group[]): Section => ({
    id: "dsp",
    title: "DSP",
    command: "",
    suite: { machine: { cpu: "", os: "", date: "" }, groups },
  });

  it("picks our biggest lead per section", () => {
    const wide = {
      ...group("higher"),
      id: "wide",
      results: [...group("higher").results, { tool: "c", version: "1", value: 20 }],
    };
    const narrow = {
      ...group("higher"),
      id: "narrow",
      results: [
        { tool: SELF, version: "1", value: 110 },
        { tool: "a", version: "1", value: 100 },
      ],
    };
    expect(highlights([section([narrow, wide])]).map((found) => found.id)).toEqual(["wide"]);
  });

  it("skips sections where we trail", () => {
    expect(highlights([section([group("lower")]), section([])])).toEqual([]);
  });
});

describe("published data", () => {
  it("has our result in every group", () => {
    for (const section of measured(SECTIONS)) {
      for (const result of section.suite.groups) {
        expect(result.results.some(ours)).toBe(true);
      }
    }
  });
});
