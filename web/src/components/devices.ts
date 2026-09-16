import { refMatches } from "../canvas/binding";
import type { DeviceInfo, DeviceRef } from "../lib/types";
import type { Options } from "./controls";

const NODE_OWNED_DRIVERS = ["recording", "siggen", "array"];

function deviceRank(device: DeviceInfo): number {
  return device.driver === "virtual" ? 1 : 0;
}

export function isVirtualDevice(device: DeviceInfo): boolean {
  return device.driver === "virtual";
}

export function rankDevices(devices: readonly DeviceInfo[]): readonly DeviceInfo[] {
  return devices.toSorted(
    (a, b) => deviceRank(a) - deviceRank(b) || a.label.localeCompare(b.label),
  );
}

export function visibleDevices(
  devices: readonly DeviceInfo[],
  showSynthetic = import.meta.env.DEV || import.meta.env.VITE_ENABLE_SYNTHETIC_DEVICES === "true",
): readonly DeviceInfo[] {
  const pickable = devices.filter((device) => !NODE_OWNED_DRIVERS.includes(device.driver));
  return rankDevices(
    showSynthetic ? pickable : pickable.filter((device) => !isVirtualDevice(device)),
  );
}

export function unclaimedDevices(
  devices: readonly DeviceInfo[],
  claimed: readonly DeviceRef[],
): readonly DeviceInfo[] {
  return devices.filter((device) => !claimed.some((reference) => refMatches(reference, device)));
}

export function groupDevices(devices: readonly DeviceInfo[]): {
  radios: readonly DeviceInfo[];
  virtual: readonly DeviceInfo[];
} {
  return {
    radios: devices.filter((device) => !isVirtualDevice(device)),
    virtual: devices.filter(isVirtualDevice),
  };
}

export type SourceTab = "radios" | "network" | "virtual";

function counted(label: string, count: number): string {
  return count > 0 ? `${label} (${count})` : label;
}

export function sourceTabs(groups: {
  radios: readonly DeviceInfo[];
  virtual: readonly DeviceInfo[];
}): Options<SourceTab> {
  const tabs: { value: SourceTab; label: string; title: string }[] = [
    { value: "radios", label: "Radios", title: "Radios attached to this machine" },
    {
      value: "network",
      label: "Network",
      title: "A radio served over rtl_tcp or SpyServer, or an AntSDR or Pluto on the network",
    },
  ];
  if (groups.virtual.length > 0) {
    tabs.push({
      value: "virtual",
      label: counted("Virtual", groups.virtual.length),
      title: "Synthetic radios for development and tests",
    });
  }
  return tabs;
}

export function deviceId(device: DeviceInfo): string {
  return `${device.driver}:${device.key}`;
}

export const NETWORK_BACKENDS = [
  { driver: "rtltcp", label: "rtl_tcp", placeholder: "192.168.1.5:1234" },
  { driver: "spyserver", label: "SpyServer", placeholder: "192.168.1.5:5555" },
  { driver: "sdrconnect", label: "SDRconnect", placeholder: "192.168.1.5:5454" },
  { driver: "ad936x", label: "AntSDR / Pluto", placeholder: "192.168.1.10:30431" },
] as const;

export function networkDeviceId(driver: string, address: string): string | null {
  const trimmed = address.trim().replace(/^[a-z][a-z0-9+._-]*:\/\//i, "");
  if (trimmed === "" || /\s/.test(trimmed)) {
    return null;
  }
  return `${driver}:${trimmed}`;
}
