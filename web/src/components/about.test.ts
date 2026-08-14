import { describe, expect, it } from "vitest";
import type { Attribution } from "../lib/types";
import {
  groupComponents,
  licenseSummary,
  matchesQuery,
  notedComponents,
  sourceLabel,
} from "./about";

const serde: Attribution = {
  name: "serde",
  version: "1.0.0",
  license: "MIT OR Apache-2.0",
  source: "rust",
  texts: ["aaaaaaaaaaaaaaaa"],
};

const codec2: Attribution = {
  name: "codec2",
  version: "0.3.1",
  license: "LGPL-2.1-only AND MIT",
  source: "rust",
  texts: [],
  note: "LGPL-2.1-only, statically linked into the binary.",
};

const react: Attribution = {
  name: "react",
  version: "19.2.8",
  license: "MIT",
  source: "web",
  texts: ["bbbbbbbbbbbbbbbb"],
};

const rtlsdr: Attribution = {
  name: "rtl-sdr (librtlsdr)",
  license: "GPL-2.0-or-later",
  source: "native",
  texts: [],
  note: "Loaded at runtime as a SoapySDR module.",
};

const ALL = [serde, codec2, react, rtlsdr];

describe("matchesQuery", () => {
  it("matches every component on an empty or blank query", () => {
    expect(matchesQuery(serde, "")).toBe(true);
    expect(matchesQuery(serde, "   ")).toBe(true);
  });

  it("matches on name, case-insensitively", () => {
    expect(matchesQuery(codec2, "CODEC")).toBe(true);
    expect(matchesQuery(codec2, "serde")).toBe(false);
  });

  /// The question a reader arrives with is "is there copyleft in here", and the answer lives
  /// in the license column rather than in any component's name.
  it("matches on the license expression", () => {
    expect(matchesQuery(rtlsdr, "gpl")).toBe(true);
    expect(matchesQuery(codec2, "lgpl")).toBe(true);
    expect(matchesQuery(react, "gpl")).toBe(false);
  });

  it("matches on the note", () => {
    expect(matchesQuery(codec2, "statically linked")).toBe(true);
  });
});

describe("groupComponents", () => {
  it("orders groups rust, web, native regardless of input order", () => {
    const groups = groupComponents([rtlsdr, react, serde], "");
    expect(groups.map((group) => group.source)).toEqual(["rust", "web", "native"]);
  });

  it("labels each group", () => {
    const groups = groupComponents([serde], "");
    expect(groups.map((group) => group.label)).toEqual([sourceLabel("rust")]);
    expect(groups.map((group) => group.label)).toEqual(["Rust crates"]);
  });

  it("drops groups a search empties instead of leaving a bare heading", () => {
    const groups = groupComponents(ALL, "gpl");
    expect(groups.map((group) => group.source)).toEqual(["rust", "native"]);
    expect(groups.map((group) => group.components)).toEqual([[codec2], [rtlsdr]]);
  });

  it("returns nothing when a search matches nothing", () => {
    expect(groupComponents(ALL, "nosuchpackage")).toEqual([]);
  });
});

describe("licenseSummary", () => {
  it("counts distinct expressions, most common first", () => {
    const summary = licenseSummary([serde, react, { ...serde, name: "serde_json" }]);
    expect(summary).toEqual([
      { license: "MIT OR Apache-2.0", count: 2 },
      { license: "MIT", count: 1 },
    ]);
  });

  it("breaks ties by license so the order is stable across builds", () => {
    const summary = licenseSummary([react, rtlsdr]);
    expect(summary.map((entry) => entry.license)).toEqual(["GPL-2.0-or-later", "MIT"]);
  });
});

describe("notedComponents", () => {
  it("keeps only the components whose license needs explaining", () => {
    expect(notedComponents(ALL)).toEqual([codec2, rtlsdr]);
  });
});
