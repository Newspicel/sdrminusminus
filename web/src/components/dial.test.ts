import { describe, expect, it } from "vitest";
import {
  dialDigits,
  dialPlaces,
  formatStep,
  inTuningRange,
  isTunable,
  parseFrequency,
  reachableHz,
  setDialDigit,
  stepDial,
  tuneTargetHz,
} from "./dial";
import { dialId } from "./FrequencyDial";

const WIDE = { min: 0, max: 6e9 };

describe("isTunable", () => {
  it("is true for a radio with somewhere to go", () => {
    expect(isTunable(WIDE)).toBe(true);
    expect(isTunable({ min: 100e6, max: 100e6 + 1 })).toBe(true);
  });

  it("is false for a range of one point", () => {
    expect(isTunable({ min: 100e6, max: 100e6 })).toBe(false);
    expect(isTunable({ min: 0, max: 0 })).toBe(false);
  });
});

describe("reachableHz", () => {
  const V4 = {
    freq_ranges: [
      { min: 500e3, max: 28.8e6 },
      { min: 24e6, max: 1.766e9 },
    ],
  };

  it("leaves a frequency a range already holds", () => {
    expect(reachableHz(V4, 7.1e6)).toBe(7.1e6);
    expect(reachableHz(V4, 145.5e6)).toBe(145.5e6);
  });

  it("moves a frequency in a gap to the nearest edge", () => {
    const HF = {
      freq_ranges: [
        { min: 1e3, max: 31e6 },
        { min: 60e6, max: 260e6 },
      ],
    };
    expect(reachableHz(HF, 40e6)).toBe(31e6);
    expect(reachableHz(HF, 55e6)).toBe(60e6);
  });

  it("clamps past either end and takes anything without ranges", () => {
    expect(reachableHz(V4, 10)).toBe(500e3);
    expect(reachableHz(V4, 9e9)).toBe(1.766e9);
    expect(reachableHz({ freq_ranges: [] }, 1.2e9)).toBe(1.2e9);
  });
});

describe("dialPlaces", () => {
  it("never renders fewer than four megahertz digits", () => {
    expect(dialPlaces(2.4e9)).toEqual([9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    expect(dialPlaces(1.7e9)).toEqual([9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    expect(dialPlaces(1.766e9)).toHaveLength(10);
    expect(dialPlaces(30e6)).toEqual([8, 7, 6, 5, 4, 3, 2, 1, 0]);
  });

  it("grows for devices that reach further, and stops growing", () => {
    expect(dialPlaces(6e9)[0]).toBe(9);
    expect(dialPlaces(1e13)[0]).toBe(11);
  });
});

describe("dialDigits", () => {
  it("splits a frequency into place-value digits", () => {
    const digits = dialDigits(100_000_000, dialPlaces(1.766e9));
    expect(digits.map((d) => d.digit)).toEqual([0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
  });

  it("marks only the zeros left of the first significant digit", () => {
    const digits = dialDigits(88_500_000, dialPlaces(1.766e9));
    expect(digits.filter((d) => d.leading).map((d) => d.place)).toEqual([9, 8]);
  });

  it("keeps the last megahertz digit lit below 1 MHz", () => {
    const digits = dialDigits(198_000, dialPlaces(30e6));
    expect(digits.slice(0, 3).map((d) => d.leading)).toEqual([true, true, false]);
  });
});

describe("stepDial", () => {
  it("steps one unit of the addressed place", () => {
    expect(stepDial(100_000_000, 6, 1, WIDE)).toBe(101_000_000);
    expect(stepDial(100_000_000, 3, -1, WIDE)).toBe(99_999_000);
  });

  it("clamps instead of carrying out of range", () => {
    expect(stepDial(5_999_000_000, 9, 1, WIDE)).toBe(6e9);
    expect(stepDial(1_000, 6, -1, { min: 24e6, max: 1.766e9 })).toBe(24e6);
  });
});

describe("setDialDigit", () => {
  it("writes one place and leaves the others alone", () => {
    expect(setDialDigit(145_500_000, 6, 8, WIDE)).toBe(148_500_000);
    expect(setDialDigit(145_500_000, 8, 0, WIDE)).toBe(45_500_000);
  });

  it("is a no-op when the digit is already there", () => {
    expect(setDialDigit(145_500_000, 5, 5, WIDE)).toBe(145_500_000);
  });

  it("clamps a write that would leave the device's range", () => {
    expect(setDialDigit(145_500_000, 9, 9, { min: 24e6, max: 1.766e9 })).toBe(1.766e9);
  });
});

describe("parseFrequency", () => {
  it("reads a bare number as megahertz", () => {
    expect(parseFrequency("145.5")).toBe(145_500_000);
    expect(parseFrequency("1090")).toBe(1_090_000_000);
  });

  it("lets a unit suffix win", () => {
    expect(parseFrequency("433800k")).toBe(433_800_000);
    expect(parseFrequency("7.1 MHz")).toBe(7_100_000);
    expect(parseFrequency("162550000 Hz")).toBe(162_550_000);
    expect(parseFrequency("2.4g")).toBe(2_400_000_000);
  });

  it("accepts a decimal comma", () => {
    expect(parseFrequency("145,5")).toBe(145_500_000);
  });

  it("returns null rather than tuning somewhere unintended", () => {
    expect(parseFrequency("")).toBeNull();
    expect(parseFrequency("abc")).toBeNull();
    expect(parseFrequency("145.5.5")).toBeNull();
    expect(parseFrequency("145 MHz extra")).toBeNull();
  });
});

describe("tuneTargetHz", () => {
  const range = { min: 24e6, max: 1.766e9 };

  it("reads what the dial's own entry reads", () => {
    expect(tuneTargetHz("145.5", range)).toBe(145_500_000);
    expect(tuneTargetHz("433800k", range)).toBe(433_800_000);
  });

  it("refuses a frequency the radio cannot reach rather than tuning the nearest edge", () => {
    expect(tuneTargetHz("10", range)).toBeNull();
    expect(tuneTargetHz("2.4g", range)).toBeNull();
    expect(tuneTargetHz("24", range)).toBe(24_000_000);
    expect(tuneTargetHz("1766", range)).toBe(1_766_000_000);
  });

  it("refuses what it cannot read", () => {
    expect(tuneTargetHz("", range)).toBeNull();
    expect(tuneTargetHz("abc", range)).toBeNull();
  });
});

describe("formatStep", () => {
  it("names each rung in the unit an operator would say", () => {
    expect(formatStep(10)).toBe("10 Hz");
    expect(formatStep(12_500)).toBe("12.5 kHz");
    expect(formatStep(100_000)).toBe("100 kHz");
    expect(formatStep(1_000_000)).toBe("1 MHz");
  });
});

describe("inTuningRange", () => {
  const range = { min: 24_000_000, max: 1_766_000_000 };

  it("passes a frequency the radio reaches and refuses one it does not", () => {
    expect(inTuningRange(145_500_000, range)).toBe(145_500_000);
    expect(inTuningRange(range.min, range)).toBe(range.min);
    expect(inTuningRange(range.max, range)).toBe(range.max);
    expect(inTuningRange(10_000_000, range)).toBeNull();
    expect(inTuningRange(2_400_000_000, range)).toBeNull();
  });
});

describe("dialId", () => {
  it("scopes a dial to the node drawing it, so two faces never collide", () => {
    expect(dialId("radio")).toBe("frequency-dial:radio");
    expect(dialId("voice")).not.toBe(dialId("radio"));
  });

  it("names each lane of a multi-stream radio apart", () => {
    expect(dialId("radio", 1)).toBe("frequency-dial:radio:1");
    expect(dialId("radio", 0)).toBe(dialId("radio"));
  });
});
