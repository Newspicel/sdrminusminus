import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DeliveryReadout, type DeliveryStatus } from "./OutputDelivery";

function readout(status: DeliveryStatus | null): string {
  return renderToStaticMarkup(<DeliveryReadout status={status} />);
}

describe("DeliveryReadout", () => {
  it("shows nothing before the first report", () => {
    expect(readout(null)).toBe("");
    expect(readout({ node: "out", delivered: 0, failed: 0, error: null })).toBe("");
  });

  it("shows a failed delivery as a fault with counts", () => {
    const html = readout({ node: "out", delivered: 3, failed: 1, error: "Webhook returned 500" });
    expect(html).toContain('role="alert"');
    expect(html).toContain("Delivery failed");
    expect(html).toContain(">Sent<");
    expect(html).toContain(">Failed<");
  });

  it("drops the fault once a delivery succeeds again", () => {
    const html = readout({ node: "out", delivered: 4, failed: 1, error: null });
    expect(html).not.toContain('role="alert"');
    expect(html).toContain(">4<");
  });
});
