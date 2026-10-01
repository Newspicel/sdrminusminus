import { describe, expect, it } from "vitest";
import type { EventOutputTarget, PatchNode } from "../../lib/types";
import { renderFace } from "../../test/faceHarness";
import { EventOutputFace } from "./EventOutputFace";

function output(target: EventOutputTarget): PatchNode {
  return { id: "out", kind: "event_output", data: { target }, position: { x: 0, y: 0 } };
}

function render(target: EventOutputTarget): string {
  const node = output(target);
  return renderFace(EventOutputFace, node, { graph: { nodes: [node], edges: [] } });
}

describe("EventOutputFace", () => {
  it("puts the service and its fields on chips", () => {
    const html = render({ service: "mqtt", broker_url: "mqtt://broker", topic: "sdr" });
    expect(html).toContain('aria-label="Output service"');
    expect(html).toContain(">MQTT<");
    expect(html).toContain('aria-label="MQTT broker URL"');
    expect(html).toContain(">mqtt://broker<");
    expect(html).not.toMatch(/<(?:select)/);
  });

  it("never shows a secret on its chip", () => {
    const html = render({ service: "webhook", url: "https://hooks.example/abc", format: "json" });
    expect(html).not.toContain("hooks.example");
    expect(html).toContain(">set<");
  });
});
