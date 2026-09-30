import { isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { PhoneAccessStatus, PhonesResponse } from "../lib/types";
import { AccessSettings, OfferCard, type PhoneActions, PhoneRows, PhonesView } from "./PhonesPanel";

const NOW = Date.parse("2026-09-29T12:00:00Z");

const ENDPOINT = {
  port: 8443,
  hosts: ["192.168.1.20:8443", "shack.local:8443"],
  pin: "3fa9c2e07b1d4a6e9f0c2b8d5e7a1c3f9b2d4e6a8c0e2f4a6b8d0c2e4f6a8b0c",
  key_check: "3FA9 C2E0 7B1D 4A6E 9F0C",
  dedicated: true,
};

const READY: PhoneAccessStatus = {
  access: { enabled: true, port: 8443 },
  listener: { state: "on", port: 8443 },
  endpoint: ENDPOINT,
  mdns: { state: "on", instance: "SDR-- shack" },
  protocol: 1,
};

const FIELD_PHONE = {
  id: "p0123456789abcdef",
  name: "Field phone",
  platform: "ios" as const,
  created_at: "2026-09-01T10:00:00Z",
  online: true,
};

const CAR_PHONE = {
  id: "p1111111111111111",
  name: "Car phone",
  platform: "android" as const,
  created_at: "2026-09-02T10:00:00Z",
  last_seen: "2026-09-29T11:55:00Z",
  online: false,
};

function actions(): PhoneActions {
  return {
    access: vi.fn(),
    pair: vi.fn(),
    cancel: vi.fn(),
    edit: vi.fn(),
    rename: vi.fn(),
    confirm: vi.fn(),
    revoke: vi.fn(),
  };
}

function data(overrides: Partial<PhonesResponse> = {}): PhonesResponse {
  return { phones: [], access: READY, ...overrides };
}

function view(response: PhonesResponse, on: PhoneActions = actions()): string {
  return renderToStaticMarkup(
    <PhonesView
      data={response}
      failed={false}
      now={NOW}
      editing={null}
      confirming={null}
      busy={false}
      on={on}
    />,
  );
}

type Props = Record<string, unknown> & { children?: ReactNode };

function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) {
    return node.flatMap(elements);
  }
  if (!isValidElement<Props>(node)) {
    return [];
  }
  return [node, ...Object.values(node.props).flatMap(elements)];
}

function pressed(node: unknown, text: string): void {
  const button = elements(node).find(
    (element) => element.props.children === text && typeof element.props.onClick === "function",
  );
  if (button === undefined) {
    throw new Error(`no button ${text}`);
  }
  (button.props.onClick as () => void)();
}

describe("PhonesPanel", () => {
  it("shows the QR, the grouped code and the countdown for an offer", () => {
    const html = view(
      data({
        offer: {
          id: "o1",
          state: { state: "live" },
          expires_at: "2026-09-29T12:04:59Z",
          failures: 0,
          code: "48210937",
          uri: "sdrmm://pair?h=192.168.1.20:8443&c=48210937&fp=3fa9&p=1",
        },
      }),
    );
    expect(html).toContain('role="img" aria-label="Pairing QR code"');
    expect(html).toContain(">4821 0937<");
    expect(html).toContain('title="Check this on the phone">3FA9 C2E0 7B1D 4A6E 9F0C<');
    expect(html).toContain(">4:59<");
    expect(html).toContain(">Cancel<");
    expect(html).not.toContain(">Pair phone<");
  });

  it("offers a new code after too many tries", () => {
    const on = actions();
    const response = data({
      offer: {
        id: "o1",
        state: { state: "burned" },
        expires_at: "2026-09-29T12:04:59Z",
        failures: 5,
      },
    });
    const html = view(response, on);
    expect(html).toContain(">Too many tries<");
    expect(html).not.toContain("Pairing QR code");
    pressed(
      OfferCard({
        view: { kind: "burned" },
        keyCheck: null,
        busy: false,
        onCancel: on.cancel,
        onNew: on.pair,
      }),
      "New code",
    );
    expect(on.pair).toHaveBeenCalledTimes(1);
  });

  it("warns when no LAN address is offered", () => {
    const html = view(
      data({
        access: { ...READY, endpoint: null, mdns: { state: "off" } },
      }),
    );
    expect(html).toContain(">No LAN address<");
    expect(html).toContain(">Discovery off<");
    expect(html).toMatch(/disabled=""[^>]*title="Allow phones first"[^>]*>Pair phone</);
  });

  it("switches phones on at the chosen port and says where they connect", () => {
    const on = actions();
    const off = data({
      access: { ...READY, access: { enabled: false, port: 9443 }, endpoint: null },
    });
    const tree = AccessSettings({ data: off, busy: false, onAccess: on.access });
    const allow = elements(tree).find((element) => element.props.label === "Allow phones");
    if (allow === undefined) {
      throw new Error("no Allow phones switch");
    }
    (allow.props.onChange as (checked: boolean) => void)(true);
    expect(on.access).toHaveBeenCalledWith({ enabled: true, port: 9443 });
    const html = view(data());
    expect(html).toContain('title="192.168.1.20:8443, shack.local:8443">Ready on 8443<');
    expect(html).toContain(">Discovery on<");
    expect(html).toContain('title="Phone port">Port<');
  });

  it("lists phones with online state and a revoke button", () => {
    const html = view(data({ phones: [FIELD_PHONE, CAR_PHONE] }));
    expect(html).toContain(">Field phone<");
    expect(html).toContain("iPhone · <span>Online</span>");
    expect(html).toContain('Android · <span title="2026-09-29T11:55:00Z">Seen 5 min ago</span>');
    expect(html).toContain('aria-label="Revoke Car phone"');
    expect(view(data())).toContain(">No phones<");
  });

  it("asks before revoking", () => {
    const on = actions();
    const rows = { phones: [FIELD_PHONE, CAR_PHONE], now: NOW, editing: null, busy: false, on };
    pressed(PhoneRows({ ...rows, confirming: null }), "Revoke");
    expect(on.confirm).toHaveBeenCalledWith(FIELD_PHONE.id);
    expect(on.revoke).not.toHaveBeenCalled();
    const confirming = PhoneRows({ ...rows, confirming: CAR_PHONE.id });
    pressed(confirming, "Revoke?");
    expect(on.revoke).toHaveBeenCalledWith(CAR_PHONE.id);
  });
});
