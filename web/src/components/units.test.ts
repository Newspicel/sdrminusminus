import { describe, expect, it } from "vitest";
import { splitUnit, unitInfo, unitTip } from "./units";

describe("unitInfo", () => {
  it("names a plain unit", () => {
    expect(unitInfo("Hz")?.name).toBe("Hertz");
    expect(unitInfo("dBFS")?.name).toBe("Decibel full scale");
  });

  it("builds a prefixed unit from its base", () => {
    expect(unitInfo("MHz")).toEqual({
      name: "Megahertz",
      about: "1,000,000 hertz. Cycles per second.",
    });
    expect(unitInfo("ms")?.name).toBe("Milliseconds");
    expect(unitInfo("µs")?.name).toBe("Microseconds");
    expect(unitInfo("MS/s")?.name).toBe("Megasamples per second");
  });

  it("knows nothing about an unknown symbol", () => {
    expect(unitInfo("wide")).toBeUndefined();
    expect(unitInfo("")).toBeUndefined();
    expect(unitInfo("xHz")).toBeUndefined();
  });
});

describe("unitTip", () => {
  it("joins name and explanation", () => {
    expect(unitTip("dB/Hz")).toBe("Decibel per hertz. Level in each hertz of bandwidth.");
  });
});

describe("splitUnit", () => {
  it("splits a trailing known unit from its value", () => {
    expect(splitUnit("446.0063 MHz")).toEqual(["446.0063", "MHz"]);
    expect(splitUnit("−42.5 dBFS")).toEqual(["−42.5", "dBFS"]);
    expect(splitUnit("12 dB SNR")).toEqual(["12", "dB SNR"]);
  });

  it("leaves text without a known trailing unit alone", () => {
    expect(splitUnit("locked")).toBeUndefined();
    expect(splitUnit("3 frames")).toBeUndefined();
    expect(splitUnit("MHz")).toBeUndefined();
  });
});
