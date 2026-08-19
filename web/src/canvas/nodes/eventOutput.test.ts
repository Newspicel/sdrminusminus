import { describe, expect, it } from "vitest";
import { eventOutputConfigured, newOutputTarget } from "./eventOutput";

const matrix = (access_token: string) =>
  eventOutputConfigured({
    service: "matrix",
    homeserver_url: "https://matrix.example",
    room_id: "!radio:matrix.example",
    access_token,
  });

const mqtt = (broker_url: string, topic: string) =>
  eventOutputConfigured({ service: "mqtt", broker_url, topic, username: "", password: "" });

describe("event output configuration", () => {
  it("needs a webhook endpoint", () => {
    expect(eventOutputConfigured({ service: "webhook", url: "", format: "json" })).toBe(false);
    expect(eventOutputConfigured({ service: "webhook", url: "   ", format: "json" })).toBe(false);
    expect(
      eventOutputConfigured({
        service: "webhook",
        url: "https://discord.com/api/webhooks/1/token",
        format: "discord",
      }),
    ).toBe(true);
  });

  it("needs all Matrix credentials", () => {
    expect(matrix("")).toBe(false);
    expect(matrix("   ")).toBe(false);
    expect(matrix("secret")).toBe(true);
  });

  it("needs an MQTT broker and a topic, but no credentials", () => {
    expect(mqtt("", "sdrmm/events")).toBe(false);
    expect(mqtt("mqtts://broker.example", "  ")).toBe(false);
    expect(mqtt("mqtts://broker.example", "sdrmm/events")).toBe(true);
  });

  it("starts every service unconfigured", () => {
    for (const service of ["webhook", "matrix", "mqtt"] as const) {
      const target = newOutputTarget(service);
      expect(target.service).toBe(service);
      expect(eventOutputConfigured(target)).toBe(false);
    }
  });
});
