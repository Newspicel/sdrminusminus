import { describe, expect, it } from "vitest";
import type {
  CodeplugChannel,
  ConversionIssue,
  ConversionReport,
  CpsJob,
  CpsPort,
  RadioModelDescriptor,
} from "../../lib/types";
import {
  anyActive,
  candidateModels,
  channelDetail,
  channelKind,
  countsLine,
  describeJob,
  formatMhz,
  formatShift,
  formatTone,
  groupIssues,
  issueLine,
  jobPercent,
  latestJob,
  reportSummary,
} from "./cps";

const fm: CodeplugChannel = {
  name: "OE1XUU",
  rx_hz: 438_950_000,
  tx_hz: 431_350_000,
  power: "high",
  rx_only: false,
  mode: "fm",
  bandwidth: "wide",
  admit: "always",
  rx_tone: { kind: "ctcss", decihertz: 1230 },
  tx_tone: { kind: "dcs", code: 23, inverted: true },
};

const dmr: CodeplugChannel = {
  name: "TG232",
  rx_hz: 439_000_000,
  tx_hz: 439_000_000,
  power: "low",
  rx_only: false,
  mode: "dmr",
  color_code: 1,
  time_slot: "two",
  admit: "color_code_free",
  contact: "Austria",
};

function job(over: Partial<CpsJob>): CpsJob {
  return {
    id: 1,
    kind: "read",
    model_id: "anytone-d890uv",
    port: "/dev/cu.usb",
    state: "running",
    step: "channels",
    done_bytes: 50,
    total_bytes: 200,
    started_at: "2026-08-23T00:00:00Z",
    ...over,
  };
}

describe("channel formatting", () => {
  it("shows the receive frequency and the shift the radio actually stores", () => {
    expect(formatMhz(fm.rx_hz)).toBe("438.95000");
    expect(formatShift(fm)).toBe("−7.6000");
    expect(formatShift(dmr)).toBe("simplex");
  });

  it("names the mode-specific settings an operator looks for", () => {
    expect(channelKind(fm)).toBe("fm");
    expect(channelKind(dmr)).toBe("dmr");
    expect(channelDetail(fm)).toBe("25 kHz  123.0 / D023I");
    expect(channelDetail(dmr)).toBe("CC1 TS2 Austria");
  });

  it("renders a missing tone as a dash rather than an empty cell", () => {
    expect(formatTone(null)).toBe("—");
    expect(formatTone(undefined)).toBe("—");
    expect(formatTone({ kind: "ctcss", decihertz: 885 })).toBe("88.5");
    expect(formatTone({ kind: "dcs", code: 23, inverted: false })).toBe("D023N");
  });
});

describe("transfer jobs", () => {
  it("reports progress against the bytes the radio owes", () => {
    expect(jobPercent(job({}))).toBe(25);
    expect(jobPercent(job({ total_bytes: 0, state: "done" }))).toBe(100);
    expect(jobPercent(job({ total_bytes: 0 }))).toBe(0);
  });

  it("says what is happening in words an operator can act on", () => {
    expect(describeJob(job({}))).toBe("Reading · channels · 25%");
    expect(describeJob(job({ kind: "write", state: "done" }))).toBe("Writing finished");
    expect(describeJob(job({ state: "failed", error: "no answer from the radio" }))).toBe(
      "no answer from the radio",
    );
    expect(describeJob(job({ state: "cancelled" }))).toBe("Reading cancelled");
  });

  it("picks the newest job and knows when the port is busy", () => {
    const jobs = [job({ id: 1, state: "done" }), job({ id: 4, state: "running" })];
    expect(latestJob(jobs)?.id).toBe(4);
    expect(anyActive(jobs)).toBe(true);
    expect(anyActive([job({ state: "done" })])).toBe(false);
    expect(latestJob([])).toBe(null);
  });
});

describe("model choice", () => {
  const models: RadioModelDescriptor[] = [
    {
      id: "radtel-rt4d",
      manufacturer: "Radtel",
      model: "RT-4D",
      family: "radtel-rt4d",
      usb: [],
      needs_explicit_selection: true,
      transfer_bytes: 1,
      limits: {} as RadioModelDescriptor["limits"],
    },
    {
      id: "anytone-d890uv",
      manufacturer: "AnyTone",
      model: "AT-D890UV",
      family: "anytone-gen2",
      usb: [{ vid: 0x0483, pid: 0x5740 }],
      needs_explicit_selection: true,
      transfer_bytes: 1,
      limits: {} as RadioModelDescriptor["limits"],
    },
  ];

  it("puts the models that claim the plugged-in port first", () => {
    const port: CpsPort = {
      port: "/dev/cu.usb",
      label: "STM32 · /dev/cu.usb",
      match_kind: "probable",
      candidate_models: ["anytone-d890uv"],
    };
    expect(candidateModels(port, models).map((model) => model.id)).toEqual([
      "anytone-d890uv",
      "radtel-rt4d",
    ]);
  });

  it("offers every model when no port is chosen", () => {
    expect(candidateModels(null, models)).toHaveLength(2);
  });
});

describe("conversion report", () => {
  const report = {
    target_model: "radtel-rt4d",
    before: {
      channels: 35,
      contacts: 1,
      group_lists: 1,
      zones: 4,
      scan_lists: 1,
      radio_ids: 2,
    },
    after: {
      channels: 35,
      contacts: 1,
      group_lists: 1,
      zones: 4,
      scan_lists: 0,
      radio_ids: 1,
    },
    issues: [
      {
        severity: "dropped" as const,
        scope: "scan_list" as const,
        item: "scan lists",
        message: "this radio has no scan lists",
      },
      {
        severity: "adjusted" as const,
        scope: "channel" as const,
        item: "OE1XUU",
        field: "power",
        message: "Max is not offered; using High",
      },
    ],
  } satisfies ConversionReport;

  it("groups what was lost above what was merely changed", () => {
    const groups = groupIssues(report);
    expect(groups.map((group) => group.severity)).toEqual(["dropped", "adjusted"]);
    expect(groups.map((group) => group.label)).toEqual(["Left behind", "Changed to fit"]);
    expect(groupIssues(null)).toEqual([]);
  });

  it("writes each issue as one readable line", () => {
    const powered: ConversionIssue = {
      severity: "adjusted",
      scope: "channel",
      item: "OE1XUU",
      field: "power",
      message: "Max is not offered; using High",
    };
    const anonymous: ConversionIssue = {
      severity: "dropped",
      scope: "scan_list",
      message: "this radio has no scan lists",
    };
    expect(issueLine(powered)).toBe("OE1XUU · power: Max is not offered; using High");
    expect(issueLine(anonymous)).toBe("this radio has no scan lists");
  });

  it("summarises the whole move in one phrase", () => {
    expect(reportSummary(report)).toBe("1 left behind, 1 changed");
    expect(reportSummary({ ...report, issues: [] })).toBe("Everything fits");
  });

  it("shows only the counts that actually moved", () => {
    expect(countsLine(report)).toBe("35 channels · 1 contacts · 4 zones · 0/1 scan lists");
  });
});
