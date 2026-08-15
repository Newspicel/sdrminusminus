// What a GPS face has to decide about the source it names, without any of the drawing. Kept out
// of `GpsFace.tsx` so that file exports only components — a mixed module costs Fast Refresh the
// component state it would otherwise preserve.
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

/** A detected port as one suggestion: the path is the value, and whatever the USB descriptor says
 * is behind it is the second line — left off, rather than repeating the path, for a port that
 * reports no identity of its own. */
export function nmeaSuggestion(device: NmeaDeviceInfo): AutocompleteSuggestion {
  const description = device.product ?? device.manufacturer;
  if (description == null) {
    return { value: device.path };
  }
  const serial = device.serial == null ? "" : ` · ${device.serial}`;
  return { value: device.path, detail: `${description}${serial}` };
}
