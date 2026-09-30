import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  askedDevices,
  lacksSerial,
  markAsked,
  nextToAsk,
  promptOff,
  turnPromptOff,
} from "./serialPrompt";
import type { DeviceInfo } from "./types";

function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => items.delete(key),
    setItem: (key, value) => items.set(key, value),
  };
}

function device(driver: string, key: string, serial?: string): DeviceInfo {
  return { driver, key, label: key, ...(serial === undefined ? {} : { serial }) };
}

describe("serial prompt", () => {
  beforeEach(() => vi.stubGlobal("localStorage", memoryStorage()));

  it("asks only about RTL-SDRs told apart by port", () => {
    expect(lacksSerial(device("rtlsdr", "1/5"))).toBe(true);
    expect(lacksSerial(device("rtlsdr", "00000123", "00000123"))).toBe(false);
    expect(lacksSerial(device("hackrf", "1/6"))).toBe(false);
  });

  it("asks once per dongle", () => {
    const attached = [device("rtlsdr", "00000007", "00000007"), device("rtlsdr", "1/5")];
    expect(nextToAsk(attached, askedDevices())?.key).toBe("1/5");
    const asked = markAsked("rtlsdr:1/5");
    expect(nextToAsk(attached, asked)).toBeUndefined();
    expect(askedDevices()).toEqual(["rtlsdr:1/5"]);
    expect(markAsked("rtlsdr:1/5")).toEqual(["rtlsdr:1/5"]);
  });

  it("stays off once turned off", () => {
    expect(promptOff()).toBe(false);
    turnPromptOff();
    expect(promptOff()).toBe(true);
  });

  it("survives a garbled store", () => {
    localStorage.setItem("sdrmm.serialPrompt.asked", "{nope");
    expect(askedDevices()).toEqual([]);
    localStorage.setItem("sdrmm.serialPrompt.asked", '["a", 3]');
    expect(askedDevices()).toEqual(["a"]);
  });
});
