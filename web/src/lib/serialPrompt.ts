import { deviceId } from "../components/devices";
import type { DeviceInfo } from "./types";

const OFF_KEY = "sdrmm.serialPrompt.off";
const ASKED_KEY = "sdrmm.serialPrompt.asked";

export function lacksSerial(device: DeviceInfo): boolean {
  return device.driver === "rtlsdr" && (device.serial ?? null) === null;
}

export function nextToAsk(
  devices: readonly DeviceInfo[],
  asked: readonly string[],
): DeviceInfo | undefined {
  return devices.find((device) => lacksSerial(device) && !asked.includes(deviceId(device)));
}

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {}
}

export function promptOff(): boolean {
  return stored(OFF_KEY) === "1";
}

export function turnPromptOff(): void {
  store(OFF_KEY, "1");
}

export function askedDevices(): string[] {
  try {
    const parsed: unknown = JSON.parse(stored(ASKED_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((id) => typeof id === "string") : [];
  } catch {
    return [];
  }
}

export function markAsked(id: string): string[] {
  const asked = [...new Set([...askedDevices(), id])];
  store(ASKED_KEY, JSON.stringify(asked));
  return asked;
}
