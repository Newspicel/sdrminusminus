// What the radio picker has to decide before it can draw anything: which devices are worth
// offering, in what order, and what string opens one. Kept out of `OpenRadio.tsx` so the picker
// file exports only components — a mixed module costs Fast Refresh the component state it would
// otherwise preserve.
import { refMatches } from "../canvas/binding";
import type { DeviceInfo, DeviceRef } from "../lib/types";

function deviceRank(device: DeviceInfo): number {
  return device.driver === "virtual" ? 1 : 0;
}

export function isRecordingDevice(device: DeviceInfo): boolean {
  return device.driver === "virtual" && device.key.startsWith("file:");
}

/** Hardware first, then the virtual devices — someone with a dongle attached should not have to
 * read past the signal generator to find it. */
export function rankDevices(devices: readonly DeviceInfo[]): readonly DeviceInfo[] {
  return devices.toSorted(
    (a, b) => deviceRank(a) - deviceRank(b) || a.label.localeCompare(b.label),
  );
}

export function visibleDevices(
  devices: readonly DeviceInfo[],
  showSynthetic = import.meta.env.DEV || import.meta.env.VITE_ENABLE_SYNTHETIC_DEVICES === "true",
): readonly DeviceInfo[] {
  return rankDevices(
    showSynthetic
      ? devices
      : devices.filter((device) => device.driver !== "virtual" || isRecordingDevice(device)),
  );
}

/** The devices still free to be named here: one radio is one open device set behind one node, so
 * the ones another node already holds are not a choice this node can make (`claimedDevices`). */
export function unclaimedDevices(
  devices: readonly DeviceInfo[],
  claimed: readonly DeviceRef[],
): readonly DeviceInfo[] {
  return devices.filter((device) => !claimed.some((reference) => refMatches(reference, device)));
}

export function groupDevices(devices: readonly DeviceInfo[]): {
  radios: readonly DeviceInfo[];
  recordings: readonly DeviceInfo[];
} {
  return {
    radios: devices.filter((device) => !isRecordingDevice(device)),
    recordings: devices.filter(isRecordingDevice),
  };
}

export function filterRecordingDevices(
  recordings: readonly DeviceInfo[],
  query: string,
): readonly DeviceInfo[] {
  const normalized = query.trim().toLowerCase();
  return normalized === ""
    ? recordings
    : recordings.filter((recording) => recording.label.toLowerCase().includes(normalized));
}

export function deviceId(device: DeviceInfo): string {
  return `${device.driver}:${device.key}`;
}

/** The protocols a radio elsewhere on the network can be reached over. Both are named, never
 * discovered — neither has any discovery — so this list is also the whole of what the picker can
 * offer before an address is typed. */
export const NETWORK_BACKENDS = [
  { driver: "rtltcp", label: "rtl_tcp", placeholder: "192.168.1.5:1234" },
  { driver: "spyserver", label: "SpyServer", placeholder: "192.168.1.5:5555" },
] as const;

/** The `driver:key` that opens a network radio, or `null` when there is nothing usable to send.
 *
 * Only the refusals that need no knowledge are made here — an empty address, one with a space in
 * it. What the key *canonicalizes* to is the server's to decide: it defaults the port and
 * lower-cases the host, and the caller learns the result back from the device the open returns.
 * Deciding it here would be a second address parser to keep in step with the backend's, and the
 * patch would then store a key the probe never reports. */
export function networkDeviceId(driver: string, address: string): string | null {
  // A pasted `rtl_tcp://host:1234` is the address with a scheme in front of it; an IPv6 literal
  // never matches, because a scheme needs the slashes. The underscore is not one a URL scheme may
  // contain, but it is what people type for this one.
  const trimmed = address.trim().replace(/^[a-z][a-z0-9+._-]*:\/\//i, "");
  if (trimmed === "" || /\s/.test(trimmed)) {
    return null;
  }
  return `${driver}:${trimmed}`;
}
