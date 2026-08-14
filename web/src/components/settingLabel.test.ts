import { describe, expect, it } from "vitest";
import { settingLabel } from "./settingLabel";

describe("settingLabel", () => {
  it("gives a snake_case driver key its words back", () => {
    expect(settingLabel("digital_agc")).toBe("digital agc");
    expect(settingLabel("offset_tune")).toBe("offset tune");
  });

  it("splits camelCase and hyphens the same way", () => {
    expect(settingLabel("biasTee")).toBe("bias Tee");
    expect(settingLabel("direct-samp")).toBe("direct samp");
  });

  it("leaves a name that is already one word alone", () => {
    expect(settingLabel("biastee")).toBe("biastee");
  });

  it("collapses repeated and edge separators rather than showing gaps", () => {
    expect(settingLabel("_rf__gain_")).toBe("rf gain");
  });
});
