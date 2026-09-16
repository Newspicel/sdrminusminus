import { refMatches } from "../canvas/binding";
import type { DeviceInfo, DeviceRef, RecordingInfo } from "../lib/types";
import type { Options } from "./controls";
import { recordingTitle } from "./recordings";

function deviceRank(device: DeviceInfo): number {
  return device.driver === "virtual" ? 1 : 0;
}

export function isRecordingDevice(device: DeviceInfo): boolean {
  return device.driver === "virtual" && device.key.startsWith("file:");
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
  return rankDevices(
    showSynthetic
      ? devices
      : devices.filter((device) => device.driver !== "virtual" || isRecordingDevice(device)),
  );
}

export function unclaimedDevices(
  devices: readonly DeviceInfo[],
  claimed: readonly DeviceRef[],
): readonly DeviceInfo[] {
  return devices.filter((device) => !claimed.some((reference) => refMatches(reference, device)));
}

function isVirtualDevice(device: DeviceInfo): boolean {
  return device.driver === "virtual" && !isRecordingDevice(device);
}

export function groupDevices(devices: readonly DeviceInfo[]): {
  radios: readonly DeviceInfo[];
  virtual: readonly DeviceInfo[];
  recordings: readonly DeviceInfo[];
} {
  return {
    radios: devices.filter((device) => device.driver !== "virtual"),
    virtual: devices.filter(isVirtualDevice),
    recordings: devices.filter(isRecordingDevice),
  };
}

export type SourceTab = "radios" | "recordings" | "network" | "virtual";

function counted(label: string, count: number): string {
  return count > 0 ? `${label} (${count})` : label;
}

export function sourceTabs(groups: {
  radios: readonly DeviceInfo[];
  virtual: readonly DeviceInfo[];
  recordings: readonly DeviceInfo[];
}): Options<SourceTab> {
  const tabs: { value: SourceTab; label: string; title: string }[] = [
    { value: "radios", label: "Radios", title: "Radios attached to this machine" },
    {
      value: "recordings",
      label: counted("Recordings", groups.recordings.length),
      title: "Saved IQ recordings, played back like a radio",
    },
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

export interface RecordingChoice {
  device: DeviceInfo;
  info: RecordingInfo | null;
  title: string;
}

export function recordingChoices(
  recordings: readonly DeviceInfo[],
  library: readonly RecordingInfo[],
): readonly RecordingChoice[] {
  const details = new Map(library.map((recording) => [recording.device_id, recording]));
  return recordings.map((device) => {
    const info = details.get(deviceId(device)) ?? null;
    return { device, info, title: info === null ? device.label : recordingTitle(info) };
  });
}

export function filterRecordingChoices(
  choices: readonly RecordingChoice[],
  query: string,
): readonly RecordingChoice[] {
  const normalized = query.trim().toLowerCase();
  if (normalized === "") {
    return choices;
  }
  return choices.filter((choice) =>
    [choice.title, choice.device.label, choice.info?.note ?? "", ...(choice.info?.tags ?? [])].some(
      (field) => field.toLowerCase().includes(normalized),
    ),
  );
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
