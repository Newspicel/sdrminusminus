import { describe, expect, it } from "vitest";
import type { ArrayNode, DeviceInfo } from "../../lib/types";
import {
  arrayMembers,
  MAX_ARRAY_MEMBERS,
  moveMember,
  withMember,
  withoutMember,
} from "./arrayNode";

function device(driver: string, key: string, label: string): DeviceInfo {
  return { driver, key, label, serial: null };
}

const ATTACHED: DeviceInfo[] = [
  device("rtlsdr", "0001", "RTL-SDR #1"),
  device("rtlsdr", "0002", "RTL-SDR #2"),
];

function array(members: string[]): ArrayNode {
  return { members, coherence: "time_sync", shared_tuning: true };
}

describe("arrayMembers", () => {
  it("keeps lane order and says which radios are actually there", () => {
    const members = arrayMembers(array(["rtlsdr:0002", "rtlsdr:0009"]), ATTACHED);
    expect(members.map((member) => member.label)).toEqual([
      "RTL-SDR #2",
      "rtlsdr:0009 (not connected)",
    ]);
    expect(members.map((member) => member.attached)).toEqual([true, false]);
  });
});

describe("member editing", () => {
  it("adds a radio once and never past the cap", () => {
    expect(withMember(["rtlsdr:0001"], "rtlsdr:0002")).toEqual(["rtlsdr:0001", "rtlsdr:0002"]);
    expect(withMember(["rtlsdr:0001"], "rtlsdr:0001")).toEqual(["rtlsdr:0001"]);
    const full = Array.from({ length: MAX_ARRAY_MEMBERS }, (_, index) => `rtlsdr:${index}`);
    expect(withMember(full, "rtlsdr:new")).toHaveLength(MAX_ARRAY_MEMBERS);
  });

  it("takes a radio out and swaps lanes without losing one", () => {
    expect(withoutMember(["a", "b"], "a")).toEqual(["b"]);
    expect(moveMember(["a", "b", "c"], 2, -1)).toEqual(["a", "c", "b"]);
    expect(moveMember(["a", "b"], 0, -1)).toEqual(["a", "b"]);
    expect(moveMember(["a", "b"], 1, 1)).toEqual(["a", "b"]);
  });
});
