import { describe, expect, it } from "vitest";
import {
  ageLabel,
  countdownLabel,
  discoveryLine,
  fixAgeLabel,
  formatPairingCode,
  headingLabel,
  listenerLine,
  offerView,
  pairedPhone,
  pairingRemaining,
  phoneStanding,
  seenLabel,
  tiltLabel,
} from "./phones";
import type { PairingOfferStatus, Phone, PhoneAccessStatus, PositionFix } from "./types";

const NOW = Date.parse("2026-09-29T12:00:00Z");

function phone(overrides: Partial<Phone> = {}): Phone {
  return {
    id: "p0123456789abcdef",
    name: "Julian's iPhone",
    platform: "ios",
    created_at: "2026-09-01T10:00:00Z",
    online: false,
    ...overrides,
  };
}

function offer(overrides: Partial<PairingOfferStatus> = {}): PairingOfferStatus {
  return {
    id: "o1",
    state: { state: "live" },
    expires_at: "2026-09-29T12:04:59Z",
    failures: 0,
    code: "48210937",
    uri: "sdrmm://pair?h=192.168.1.20:8443&c=48210937&fp=ab&p=1",
    ...overrides,
  };
}

function fix(overrides: Partial<PositionFix> = {}): PositionFix {
  return { latitude: 52.52, longitude: 13.405, time: "2026-09-29T11:59:57Z", ...overrides };
}

function access(overrides: Partial<PhoneAccessStatus> = {}): PhoneAccessStatus {
  return {
    access: { enabled: true, port: 8443 },
    listener: { state: "on", port: 8443 },
    mdns: { state: "off" },
    protocol: 1,
    ...overrides,
  };
}

describe("phones", () => {
  it("groups the pairing code", () => {
    expect(formatPairingCode("48210937")).toBe("4821 0937");
    expect(formatPairingCode("123456")).toBe("123456");
  });

  it("counts down and never below zero", () => {
    expect(pairingRemaining("2026-09-29T12:04:59Z", NOW)).toBe(299_000);
    expect(pairingRemaining("2026-09-29T11:59:00Z", NOW)).toBe(0);
    expect(countdownLabel(299_000)).toBe("4:59");
    expect(countdownLabel(59_001)).toBe("1:00");
    expect(countdownLabel(-5_000)).toBe("0:00");
  });

  it("says online or when last seen", () => {
    expect(seenLabel(phone({ online: true }), NOW)).toBe("Online");
    expect(seenLabel(phone({ last_seen: "2026-09-29T11:55:00Z" }), NOW)).toBe("Seen 5 min ago");
    expect(seenLabel(phone(), NOW)).toBe("Never");
    expect(ageLabel(3_400)).toBe("3 s");
    expect(ageLabel(2 * 3_600_000)).toBe("2 h");
    expect(ageLabel(3 * 86_400_000)).toBe("3 d");
    expect(fixAgeLabel("2026-09-29T11:59:57Z", NOW)).toBe("3 s");
    expect(fixAgeLabel(null, NOW)).toBe("-");
  });

  it("labels heading with its source and accuracy", () => {
    expect(headingLabel(null)).toBe("-");
    expect(headingLabel(fix())).toBe("-");
    expect(headingLabel(fix({ heading_deg: 91, heading_source: "compass" }))).toBe("091° compass");
    expect(
      headingLabel(fix({ heading_deg: 123.4, heading_source: "course", heading_accuracy_deg: 5 })),
    ).toBe("123° GPS course ±5°");
    expect(headingLabel(fix({ heading_deg: 359.7 }))).toBe("000°");
    expect(headingLabel(fix({ heading_deg: -4 }))).toBe("356°");
    expect(tiltLabel(fix())).toBeNull();
    expect(tiltLabel(fix({ pitch_deg: 2.2, roll_deg: -1.4 }))).toBe("2° / -1°");
  });

  it("finds the phone that just paired", () => {
    const paired = phone({ id: "p1111111111111111", name: "E2E phone" });
    const phones = [phone(), paired];
    expect(pairedPhone(offer({ state: { state: "used", phone: paired.id } }), phones)).toBe(paired);
    expect(pairedPhone(offer(), phones)).toBeNull();
    expect(pairedPhone(null, phones)).toBeNull();
  });

  it("shows a live offer, then says it expired or burned for a while", () => {
    expect(offerView(offer(), NOW)).toEqual({
      kind: "live",
      code: "48210937",
      uri: "sdrmm://pair?h=192.168.1.20:8443&c=48210937&fp=ab&p=1",
      remainingMs: 299_000,
    });
    const later = NOW + 6 * 60_000;
    expect(offerView(offer(), later)).toEqual({ kind: "expired" });
    expect(offerView(offer({ state: { state: "burned" } }), NOW)).toEqual({ kind: "burned" });
    expect(offerView(offer({ state: { state: "expired" } }), NOW + 3_600_000)).toBeNull();
    expect(offerView(offer({ state: { state: "used", phone: "p1" } }), NOW)).toBeNull();
    expect(offerView(offer({ state: { state: "cancelled" } }), NOW)).toBeNull();
  });

  it("tells online, offline and not paired apart", () => {
    const phones = [phone({ online: true })];
    expect(phoneStanding(phones, "p0123456789abcdef", null)).toBe("online");
    expect(phoneStanding(phones, "p0123456789abcdef", "phone offline")).toBe("offline");
    expect(phoneStanding([phone()], "p0123456789abcdef", null)).toBe("offline");
    expect(phoneStanding(phones, "p9999999999999999", null)).toBe("not_paired");
    expect(phoneStanding(undefined, "p0123456789abcdef", "phone not paired")).toBe("not_paired");
    expect(phoneStanding(undefined, "p0123456789abcdef", null)).toBeNull();
  });

  it("states where phones connect", () => {
    expect(
      listenerLine(
        access({
          endpoint: {
            port: 8443,
            hosts: ["192.168.1.20:8443", "shack.local:8443"],
            pin: "ab",
            key_check: "AB",
            dedicated: true,
          },
        }),
      ),
    ).toEqual({ text: "Ready on 8443", tone: "ok", title: "192.168.1.20:8443, shack.local:8443" });
    expect(
      listenerLine(access({ listener: { state: "failed", port: 8443, reason: "port in use" } })),
    ).toMatchObject({ text: "Port in use", tone: "danger" });
    expect(
      listenerLine(access({ listener: { state: "failed", port: 8443, reason: "no key" } })).text,
    ).toBe("Failed");
    expect(listenerLine(access()).text).toBe("No LAN address");
    expect(listenerLine(access({ listener: { state: "off" } })).text).toBe("Off");
    expect(discoveryLine(access()).text).toBe("Discovery off");
    expect(discoveryLine(access({ mdns: { state: "failed", reason: "no socket" } }))).toEqual({
      text: "Discovery failed",
      tone: "danger",
      title: "no socket",
    });
  });
});
