import type { AutocompleteSuggestion } from "../../components/TextAutocomplete";
import type { NmeaDeviceInfo } from "../../lib/types";

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
