import { describe, expect, it } from "vitest";
import { chatOutputConfigured } from "./chatOutput";

describe("chat output configuration", () => {
  it("needs a Discord webhook URL", () => {
    expect(chatOutputConfigured({ service: "discord", webhook_url: "" })).toBe(false);
    expect(
      chatOutputConfigured({
        service: "discord",
        webhook_url: "https://discord.com/api/webhooks/1/token",
      }),
    ).toBe(true);
  });

  it("needs all Matrix credentials", () => {
    expect(
      chatOutputConfigured({
        service: "matrix",
        homeserver_url: "https://matrix.example",
        room_id: "!radio:matrix.example",
        access_token: "",
      }),
    ).toBe(false);
    expect(
      chatOutputConfigured({
        service: "matrix",
        homeserver_url: "https://matrix.example",
        room_id: "!radio:matrix.example",
        access_token: "secret",
      }),
    ).toBe(true);
  });
});
