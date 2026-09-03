import type { AutocompleteSuggestion } from "../../components/TextAutocomplete";
import type { NmeaDeviceInfo, PositionSource } from "../../lib/types";

export function validGpsdAddress(address: string): boolean {
  const separator = address.lastIndexOf(":");
  if (separator <= 0) {
    return false;
  }
  const host = address.slice(0, separator);
  const port = Number(address.slice(separator + 1));
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    return false;
  }
  if (host.startsWith("[") || host.endsWith("]")) {
    try {
      const parsed = new URL(`http://${host}`);
      return parsed.hostname.startsWith("[") && parsed.hostname.endsWith("]");
    } catch {
      return false;
    }
  }
  return /^[a-z0-9._-]+$/i.test(host);
}

export function nmeaSuggestion(device: NmeaDeviceInfo): AutocompleteSuggestion {
  const description = device.product ?? device.manufacturer;
  if (description == null) {
    return { value: device.path };
  }
  const serial = device.serial == null ? "" : ` · ${device.serial}`;
  return { value: device.path, detail: `${description}${serial}` };
}

export function nmeaDetail(device: NmeaDeviceInfo): string {
  return [device.product ?? device.manufacturer, device.serial]
    .filter((part): part is string => part != null && part !== "")
    .join(" · ");
}

export function filterNmeaDevices(
  devices: readonly NmeaDeviceInfo[],
  query: string,
): readonly NmeaDeviceInfo[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") {
    return devices;
  }
  return devices.filter((device) =>
    [device.path, device.product, device.manufacturer, device.serial]
      .filter((part): part is string => part != null)
      .some((part) => part.toLowerCase().includes(needle)),
  );
}

export const DEFAULT_NMEA_BAUD = 9_600;
export const DEFAULT_NMEA_INTERVAL_MS = 1_000;

export function nmeaSource(path: string): PositionSource {
  return {
    type: "nmea",
    device: path,
    baud: DEFAULT_NMEA_BAUD,
    update_interval_ms: DEFAULT_NMEA_INTERVAL_MS,
  };
}
