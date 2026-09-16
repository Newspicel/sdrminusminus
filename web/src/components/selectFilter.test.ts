import { describe, expect, it } from "vitest";
import { optionMatches } from "./selectFilter";

const ADSB = { value: "adsb", label: "ADS-B" };
const CTCSS = { value: "nfm_ctcss", label: "NFM voice · CTCSS 88.5" };
const DVBS2 = { value: "dvbs2", label: "DVB-S2" };

describe("optionMatches", () => {
  it("matches the name however the mode is punctuated", () => {
    expect(optionMatches(ADSB, "adsb")).toBe(true);
    expect(optionMatches(ADSB, "ADS-B")).toBe(true);
    expect(optionMatches(ADSB, "ads b")).toBe(true);
    expect(optionMatches(DVBS2, "dvb s2")).toBe(true);
    expect(optionMatches(DVBS2, "DVBS2")).toBe(true);
  });

  it("matches the id a decoder is known by as well as its label", () => {
    expect(optionMatches(CTCSS, "nfm_ctcss")).toBe(true);
    expect(optionMatches(CTCSS, "ctcss")).toBe(true);
    expect(optionMatches(CTCSS, "88.5")).toBe(true);
  });

  it("matches part of a word, not only its start", () => {
    expect(optionMatches(CTCSS, "voice")).toBe(true);
  });

  it("offers everything for an empty or blank query", () => {
    expect(optionMatches(ADSB, "")).toBe(true);
    expect(optionMatches(ADSB, "   ")).toBe(true);
    expect(optionMatches(ADSB, "-·-")).toBe(true);
  });

  it("offers nothing that does not match", () => {
    expect(optionMatches(ADSB, "pocsag")).toBe(false);
    expect(optionMatches(CTCSS, "dvbt")).toBe(false);
  });
});
