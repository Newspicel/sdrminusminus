import { describe, expect, it } from "vitest";
import { DMR_TRUNK_PROTOCOLS, dmrTrunkGuidance } from "./DmrTrunkFace";

describe("DMR trunk protocols", () => {
  it("offers Hytera XPT alongside the Motorola and Tier III systems", () => {
    expect(DMR_TRUNK_PROTOCOLS).toEqual([
      { value: "auto", label: "Auto-detect" },
      { value: "capacity_plus", label: "Capacity Plus" },
      { value: "hytera_xpt", label: "Hytera XPT" },
      { value: "tier_three", label: "Tier III / Capacity Max" },
    ]);
  });

  it("explains that every XPT repeater output must be connected", () => {
    expect(dmrTrunkGuidance("hytera_xpt")).toContain("every Hytera XPT repeater output frequency");
    expect(dmrTrunkGuidance("auto")).toContain("Hytera XPT");
    expect(dmrTrunkGuidance("auto", "hytera_xpt")).toContain("Detected Hytera XPT signalling");
  });
});
