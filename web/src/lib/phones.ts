import type {
  HeadingSource,
  PairingOfferStatus,
  Phone,
  PhoneAccessStatus,
  PhonePlatform,
  PositionFix,
} from "./types";

export const HEADING_SOURCE_LABEL: Record<HeadingSource, string> = {
  compass: "compass",
  course: "GPS course",
  fused: "fused",
  gnss: "GNSS",
  sensor: "sensor",
};

export const PLATFORM_LABEL: Record<PhonePlatform, string> = {
  ios: "iPhone",
  android: "Android",
};

export const PHONE_OFFLINE = "phone offline";
export const PHONE_NOT_PAIRED = "phone not paired";
export const PORT_IN_USE = "port in use";
export const OFFER_LINGER_MS = 10 * 60_000;
export const OFFER_POLL_MS = 2_000;

export type PhoneStanding = "online" | "offline" | "not_paired";

export const STANDING_LABEL: Record<PhoneStanding, string> = {
  online: "Online",
  offline: "Offline",
  not_paired: "Not paired",
};

export type OfferView =
  | { kind: "live"; code: string; uri: string | null; remainingMs: number }
  | { kind: "burned" }
  | { kind: "expired" };

export type Tone = "ok" | "danger" | "dim";

export interface StatusLine {
  text: string;
  tone: Tone;
  title?: string;
}

export function formatPairingCode(code: string): string {
  return /^\d{8}$/.test(code) ? `${code.slice(0, 4)} ${code.slice(4)}` : code;
}

export function pairingRemaining(expiresAt: string, now: number): number {
  const at = Date.parse(expiresAt);
  return Number.isFinite(at) ? Math.max(0, at - now) : 0;
}

export function countdownLabel(ms: number): string {
  const seconds = Number.isFinite(ms) ? Math.max(0, Math.ceil(ms / 1_000)) : 0;
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function ageLabel(ms: number): string {
  if (!Number.isFinite(ms)) {
    return "-";
  }
  const seconds = Math.max(0, Math.floor(ms / 1_000));
  if (seconds < 60) {
    return `${seconds} s`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes} min`;
  }
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? `${hours} h` : `${Math.floor(hours / 24)} d`;
}

export function seenLabel(phone: Phone, now: number): string {
  if (phone.online) {
    return "Online";
  }
  const at = phone.last_seen == null ? Number.NaN : Date.parse(phone.last_seen);
  return Number.isFinite(at) ? `Seen ${ageLabel(now - at)} ago` : "Never";
}

export function fixAgeLabel(time: string | null | undefined, now: number): string {
  const at = time == null ? Number.NaN : Date.parse(time);
  return Number.isFinite(at) ? ageLabel(now - at) : "-";
}

export function headingLabel(fix: PositionFix | null | undefined): string {
  const heading = fix?.heading_deg;
  if (fix == null || heading == null) {
    return "-";
  }
  const degrees = String(((Math.round(heading) % 360) + 360) % 360).padStart(3, "0");
  const source = fix.heading_source == null ? "" : ` ${HEADING_SOURCE_LABEL[fix.heading_source]}`;
  const accuracy =
    fix.heading_accuracy_deg == null ? "" : ` ±${Math.round(fix.heading_accuracy_deg)}°`;
  return `${degrees}°${source}${accuracy}`;
}

function angle(value: number | null | undefined): string {
  return value == null ? "-" : `${Math.round(value)}°`;
}

export function tiltLabel(fix: PositionFix | null | undefined): string | null {
  if (fix == null || (fix.pitch_deg == null && fix.roll_deg == null)) {
    return null;
  }
  return `${angle(fix.pitch_deg)} / ${angle(fix.roll_deg)}`;
}

export function phoneStanding(
  phones: readonly Phone[] | undefined,
  id: string,
  error: string | null | undefined,
): PhoneStanding | null {
  if (error === PHONE_NOT_PAIRED) {
    return "not_paired";
  }
  if (phones === undefined) {
    return error === PHONE_OFFLINE ? "offline" : null;
  }
  const phone = phones.find((candidate) => candidate.id === id);
  if (phone === undefined) {
    return "not_paired";
  }
  return phone.online && error !== PHONE_OFFLINE ? "online" : "offline";
}

export function offerView(
  offer: PairingOfferStatus | null | undefined,
  now: number,
): OfferView | null {
  if (offer == null) {
    return null;
  }
  const recent = now - Date.parse(offer.expires_at) < OFFER_LINGER_MS;
  switch (offer.state.state) {
    case "live": {
      const remainingMs = pairingRemaining(offer.expires_at, now);
      if (remainingMs > 0 && offer.code != null) {
        return { kind: "live", code: offer.code, uri: offer.uri ?? null, remainingMs };
      }
      return recent ? { kind: "expired" } : null;
    }
    case "expired":
      return recent ? { kind: "expired" } : null;
    case "burned":
      return recent ? { kind: "burned" } : null;
    default:
      return null;
  }
}

export function offerIsLive(offer: PairingOfferStatus | null | undefined): boolean {
  return offer?.state.state === "live";
}

export function pairedPhone(
  offer: PairingOfferStatus | null | undefined,
  phones: readonly Phone[],
): Phone | null {
  if (offer?.state.state !== "used") {
    return null;
  }
  const id = offer.state.phone;
  return phones.find((phone) => phone.id === id) ?? null;
}

export function listenerLine(access: PhoneAccessStatus): StatusLine {
  if (access.endpoint != null) {
    return {
      text: `Ready on ${access.endpoint.port}`,
      tone: "ok",
      title: access.endpoint.hosts.join(", "),
    };
  }
  switch (access.listener.state) {
    case "failed":
      return {
        text: access.listener.reason === PORT_IN_USE ? "Port in use" : "Failed",
        tone: "danger",
        title: access.listener.reason,
      };
    case "on":
      return {
        text: "No LAN address",
        tone: "danger",
        title: "Connect this computer to a network",
      };
    case "off":
      return { text: "Off", tone: "dim" };
  }
}

export function discoveryLine(access: PhoneAccessStatus): StatusLine {
  switch (access.mdns.state) {
    case "on":
      return { text: "Discovery on", tone: "ok", title: access.mdns.instance };
    case "failed":
      return { text: "Discovery failed", tone: "danger", title: access.mdns.reason };
    case "off":
      return { text: "Discovery off", tone: "dim" };
  }
}
