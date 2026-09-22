import { describe, expect, it } from "vitest";
import { due, frameTrack, lapped } from "./timeline";

function frame(seq: number, timestamp: bigint): string {
  const bytes = new Uint8Array(20);
  const view = new DataView(bytes.buffer);
  view.setUint8(0, 1);
  view.setUint16(2, 7, true);
  view.setUint32(4, seq, true);
  view.setBigUint64(8, timestamp, true);
  return btoa(String.fromCharCode(...bytes));
}

describe("due", () => {
  const items = [{ at: 0 }, { at: 400 }, { at: 900 }];

  it("returns the items inside a window", () => {
    expect(due(items, 1000, 0, 500).map((entry) => entry.item.at)).toEqual([0, 400]);
  });

  it("wraps into the next lap", () => {
    expect(due(items, 1000, 800, 1500)).toEqual([
      { item: { at: 900 }, lap: 0 },
      { item: { at: 0 }, lap: 1 },
      { item: { at: 400 }, lap: 1 },
    ]);
  });

  it("returns nothing for an empty window", () => {
    expect(due(items, 1000, 300, 300)).toEqual([]);
  });
});

describe("lapped", () => {
  it("keeps seq and timestamp increasing across laps", () => {
    const track = frameTrack([
      { at: 0, binary: frame(10, 960n) },
      { at: 20, binary: frame(11, 1920n) },
      { at: 40, binary: frame(12, 2880n) },
    ]);
    const first = track.frames[0];
    if (first === undefined) {
      throw new Error("a frame");
    }
    const view = new DataView(lapped(track, first, 1));
    expect(view.getUint32(4, true)).toBe(13);
    expect(view.getBigUint64(8, true)).toBe(3840n);
    expect(view.getUint16(2, true)).toBe(7);
  });

  it("leaves the first lap untouched", () => {
    const track = frameTrack([{ at: 0, binary: frame(5, 100n) }]);
    const first = track.frames[0];
    if (first === undefined) {
      throw new Error("a frame");
    }
    expect(new DataView(lapped(track, first, 0)).getUint32(4, true)).toBe(5);
  });
});
