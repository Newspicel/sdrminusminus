import { describe, expect, it } from "vitest";
import type { DeviceSettings } from "../lib/types";
import { followOffset } from "./converter";

describe("followOffset", () => {
  it("moves what is shown when only the offset changes", () => {
    const followed = followOffset({ center_hz: 100e6 }, { offset_hz: 9.75e9 });
    expect(followed.center_hz).toBe(9.85e9);
  });

  it("moves relative to the offset already in place", () => {
    const followed = followOffset({ center_hz: 9.85e9, offset_hz: 9.75e9 }, { offset_hz: 10.6e9 });
    expect(followed.center_hz).toBe(10.7e9);
  });

  it("leaves a delta alone that names a frequency or changes no offset", () => {
    const tuned: DeviceSettings = { center_hz: 1e9, offset_hz: 9.75e9 };
    expect(followOffset({ center_hz: 100e6 }, tuned)).toBe(tuned);
    const gain: DeviceSettings = { gains: [{ stage: "LNA", value_db: 10 }] };
    expect(followOffset({ center_hz: 100e6 }, gain)).toBe(gain);
  });
});
