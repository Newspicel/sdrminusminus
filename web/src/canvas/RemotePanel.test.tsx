import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { RemoteStatus } from "../lib/types";
import { RemoteView } from "./RemotePanel";

function render(patch: Partial<RemoteStatus>): string {
  const status: RemoteStatus = {
    state: "unpaired",
    app_origin: "https://app.sdrmm.com",
    via_relay: false,
    ...patch,
  };
  return renderToStaticMarkup(
    <RemoteView status={status} busy={false} onPair={() => {}} onUnpair={() => {}} />,
  );
}

describe("RemoteView", () => {
  it("offers to connect when unpaired", () => {
    expect(render({})).toContain("Connect to app.sdrmm.com");
  });

  it("shows the code, a QR and the approval link while pairing", () => {
    const html = render({
      state: "pairing",
      user_code: "BCDF-GHJK",
      verification_uri_complete: "https://app.sdrmm.com/pair?code=BCDF-GHJK",
    });
    expect(html).toContain("BCDF-GHJK");
    expect(html).toContain("Pairing QR code");
    expect(html).toContain('href="https://app.sdrmm.com/pair?code=BCDF-GHJK"');
    expect(html).toContain("Cancel");
  });

  it("shows the status and a way out once paired", () => {
    const html = render({ state: "online", device_id: "abc" });
    expect(html).toContain("Online");
    expect(html).toContain("Open app.sdrmm.com");
    expect(html).toContain("Disconnect");
  });

  it("keeps pairing controls off a page opened through the relay", () => {
    const html = render({ state: "online", via_relay: true });
    expect(html).toContain("Online");
    expect(html).not.toContain("Disconnect");
  });

  it("explains a removed device and offers to pair again", () => {
    const html = render({ state: "rejected", error: "device removed" });
    expect(html).toContain("Disconnected: device removed");
    expect(html).toContain("Pair again");
  });
});
