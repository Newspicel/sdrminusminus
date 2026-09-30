import { describe, expect, it } from "vitest";
import type { TemplateInfo } from "../lib/types";
import { templateSize } from "./templates";

function template(extra: Partial<TemplateInfo>): TemplateInfo {
  return {
    id: "t",
    name: "T",
    description: "",
    explainer: "",
    center_hz: 100e6,
    sample_rate: 2.4e6,
    channels: [],
    min_freq_hz: 100e6,
    max_freq_hz: 100e6,
    ...extra,
  };
}

describe("templateSize", () => {
  it("counts lanes for an array template and channels otherwise", () => {
    expect(templateSize(template({ min_lanes: 5 }))).toBe("5 lanes");
    expect(templateSize(template({}))).toBe("0 ch");
  });
});
