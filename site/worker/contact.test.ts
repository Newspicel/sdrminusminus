import { describe, expect, it } from "vitest";
import { CONTACT_PATH } from "../src/contact";
import worker from "./index";

interface Options {
  allowed?: boolean;
  failing?: boolean;
}

function environment({ allowed = true, failing = false }: Options = {}) {
  const sent: EmailMessageBuilder[] = [];
  const env = {
    CONTACT_TO: "hi@jhaag.me",
    CONTACT_FROM: "contact@sdrmm.com",
    CONTACT_LIMIT: { limit: async () => ({ success: allowed }) },
    EMAIL: {
      send: async (message: EmailMessageBuilder) => {
        if (failing) {
          throw Object.assign(new Error("not verified"), { code: "E_SENDER_NOT_VERIFIED" });
        }
        sent.push(message);
        return { messageId: "1" };
      },
    },
    ASSETS: { fetch: async () => new Response(null, { status: 404 }) },
  } as unknown as Env;
  return { env, sent };
}

const valid = { name: "Ada", email: "ada@example.org", message: "Need a decoder." };

function post(fields: Record<string, string>, headers: Record<string, string> = {}) {
  return new Request(`https://sdrmm.com${CONTACT_PATH}`, {
    method: "POST",
    body: new URLSearchParams(fields),
    headers: { accept: "application/json", origin: "https://sdrmm.com", ...headers },
  });
}

async function send(request: Request, env: Env) {
  return worker.fetch(request as Parameters<typeof worker.fetch>[0], env);
}

describe("contact form", () => {
  it("mails a valid message to the owner, with the visitor as reply-to", async () => {
    const { env, sent } = environment();
    const response = await send(post(valid), env);
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ outcome: "sent" });
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({
      to: "hi@jhaag.me",
      from: { email: "contact@sdrmm.com" },
      replyTo: { email: "ada@example.org", name: "Ada" },
      subject: "SDR--: Ada",
    });
    expect(sent[0]?.text).toContain("Need a decoder.");
  });

  it("redirects a plain form post back to the page with the outcome", async () => {
    const { env } = environment();
    const response = await send(post(valid, { accept: "text/html" }), env);
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/business.html#sent");
  });

  it("rejects an invalid message without sending", async () => {
    const { env, sent } = environment();
    const response = await send(post({ ...valid, email: "ada" }), env);
    expect(response.status).toBe(400);
    expect(await response.json()).toEqual({ outcome: "invalid" });
    expect(sent).toHaveLength(0);
  });

  it("rejects oversized bodies without reading them", async () => {
    const { env, sent } = environment();
    const response = await send(post(valid, { "content-length": "70000" }), env);
    expect(response.status).toBe(400);
    expect(sent).toHaveLength(0);
  });

  it("drops a message with the trap field filled", async () => {
    const { env, sent } = environment();
    const response = await send(post({ ...valid, website: "spam" }), env);
    expect(await response.json()).toEqual({ outcome: "sent" });
    expect(sent).toHaveLength(0);
  });

  it("turns away a sender over the rate limit", async () => {
    const { env, sent } = environment({ allowed: false });
    const response = await send(post(valid), env);
    expect(response.status).toBe(429);
    expect(await response.json()).toEqual({ outcome: "busy" });
    expect(sent).toHaveLength(0);
  });

  it("reports a failed send", async () => {
    const { env } = environment({ failing: true });
    const response = await send(post(valid), env);
    expect(response.status).toBe(502);
    expect(await response.json()).toEqual({ outcome: "failed" });
  });

  it("refuses posts from other sites", async () => {
    const { env, sent } = environment();
    const response = await send(post(valid, { origin: "https://evil.example" }), env);
    expect(response.status).toBe(403);
    expect(sent).toHaveLength(0);
  });

  it("only accepts POST", async () => {
    const { env } = environment();
    const response = await send(new Request(`https://sdrmm.com${CONTACT_PATH}`), env);
    expect(response.status).toBe(405);
  });
});
