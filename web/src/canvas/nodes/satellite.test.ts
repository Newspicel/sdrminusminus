import { describe, expect, it } from "vitest";
import {
  compass,
  formatDoppler,
  passLine,
  pastedElements,
  shownSignals,
  transmitterLabel,
} from "./satellite";

const ISS = `ISS (ZARYA)
1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999
2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041`;

describe("satellite face", () => {
  it("names the compass point nearest an azimuth", () => {
    expect(compass(0)).toBe("N");
    expect(compass(359)).toBe("N");
    expect(compass(92)).toBe("E");
    expect(compass(-45)).toBe("NW");
  });

  it("writes a Doppler shift with its sign and a readable unit", () => {
    expect(formatDoppler(9_876)).toBe("+9.88 kHz");
    expect(formatDoppler(-420)).toBe("−420 Hz");
    expect(formatDoppler(0)).toBe("0 Hz");
  });

  it("tells a pass to come from one underway and one that never ends", () => {
    expect(
      passLine({ aos: 1_090, los: 1_600, max_elevation_deg: 42.4, max_at: 1_300 }, 1_000),
    ).toBe("in 1:30, up to 42°");
    expect(passLine({ aos: 900, los: 1_600, max_elevation_deg: 12, max_at: 1_300 }, 1_000)).toBe(
      "sets in 10:00, up to 12°",
    );
    expect(passLine({ max_elevation_deg: 30, max_at: 1_000 }, 1_000)).toBe("always up");
    expect(passLine(null, 1_000)).toBe("none in 48 h");
  });

  it("recognises pasted element sets with or without a name", () => {
    expect(pastedElements(ISS)).toBe(ISS);
    expect(pastedElements(ISS.split("\n").slice(1).join("\n"))).toBe(
      ISS.split("\n").slice(1).join("\n"),
    );
    expect(pastedElements("ISS")).toBeNull();
  });

  it("labels a transmitter by what it is and where it sits", () => {
    expect(
      transmitterLabel({
        id: "a",
        description: "FM voice",
        mode: "FM",
        downlink_hz: 437_800_000,
        alive: true,
      }),
    ).toBe("FM voice FM 437.800");
    expect(transmitterLabel({ id: "b", description: "Beacon", alive: false })).toBe("Beacon (off)");
  });

  it("offers live signals, and a dead one only while it is the one picked", () => {
    const signals = [
      { id: "live", description: "Voice", downlink_hz: 437_800_000, alive: true },
      { id: "dead", description: "Old beacon", downlink_hz: 145_800_000, alive: false },
      { id: "blank", description: "No downlink", alive: true },
    ];
    expect(shownSignals(signals, null).map((signal) => signal.id)).toEqual(["live"]);
    expect(shownSignals(signals, "dead").map((signal) => signal.id)).toEqual(["live", "dead"]);
  });
});
