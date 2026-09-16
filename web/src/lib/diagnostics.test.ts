import { beforeEach, describe, expect, it } from "vitest";
import {
  clientEvents,
  describeError,
  droppedEvents,
  type ErrorSource,
  installGlobalHandlers,
  recordEvent,
  redactText,
  resetEvents,
} from "./diagnostics";

beforeEach(() => {
  resetEvents();
});

describe("the client ring", () => {
  it("keeps the newest events and counts what it dropped", () => {
    for (let index = 0; index < 250; index += 1) {
      recordEvent("error", "test", `event ${index}`);
    }
    const kept = clientEvents();
    expect(kept).toHaveLength(200);
    expect(kept[0]?.message).toBe("event 50");
    expect(kept.at(-1)?.message).toBe("event 249");
    expect(droppedEvents()).toBe(50);
  });

  it("stamps each event with its level and source", () => {
    recordEvent("warn", "socket", "lost the server");
    expect(clientEvents()[0]).toMatchObject({ level: "warn", source: "socket" });
    expect(clientEvents()[0]?.at).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("truncates a message that would swamp the report", () => {
    recordEvent("error", "test", "x".repeat(5000));
    expect(clientEvents()[0]?.message).toHaveLength(2001);
    expect(clientEvents()[0]?.message.endsWith("…")).toBe(true);
  });
});

describe("redaction", () => {
  it("strips the shared token from a url", () => {
    expect(redactText("ws://host/api/ws?token=s3cr3t-value")).toBe(
      "ws://host/api/ws?token=<redacted>",
    );
    expect(redactText("GET /api/state?x=1&access_token=abc123 failed")).toBe(
      "GET /api/state?x=1&access_token=<redacted> failed",
    );
  });

  it("strips a bearer token", () => {
    expect(redactText("Authorization: Bearer abc.def.ghi")).toBe(
      "Authorization: Bearer <redacted>",
    );
  });

  it("masks a lan address but keeps the loopback", () => {
    expect(redactText("connecting to 192.168.1.20:8080")).toBe("connecting to <ip>:8080");
    expect(redactText("connecting to 127.0.0.1:8080")).toBe("connecting to 127.0.0.1:8080");
    expect(redactText("bound 0.0.0.0:8080")).toBe("bound 0.0.0.0:8080");
  });

  it("redacts what it records", () => {
    recordEvent("error", "socket", "ws://host/api/ws?token=s3cr3t");
    expect(clientEvents()[0]?.message).toBe("ws://host/api/ws?token=<redacted>");
  });
});

describe("describeError", () => {
  it("prefers a stack when there is one", () => {
    const error = new Error("boom");
    expect(describeError(error)).toContain("boom");
  });

  it("falls back to the name and message without a stack", () => {
    const error = new Error("boom");
    error.stack = "";
    expect(describeError(error)).toBe("Error: boom");
  });

  it("handles a thrown non-error", () => {
    expect(describeError("plain string")).toBe("plain string");
    expect(describeError({ reason: "nope" })).toBe('{"reason":"nope"}');
    expect(describeError(undefined)).toBe("undefined");
  });

  it("survives a value that cannot be serialized", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(describeError(cyclic)).toBe("[object Object]");
  });
});

function fakeWindow() {
  const handlers = new Map<string, ((event: Event) => void)[]>();
  const target: ErrorSource = {
    addEventListener(type, handler) {
      handlers.set(type, [...(handlers.get(type) ?? []), handler]);
    },
    removeEventListener(type, handler) {
      handlers.set(
        type,
        (handlers.get(type) ?? []).filter((known) => known !== handler),
      );
    },
  };
  const dispatch = (type: string, event: Record<string, unknown>) => {
    for (const handler of handlers.get(type) ?? []) {
      handler(event as unknown as Event);
    }
  };
  return { target, dispatch };
}

describe("global handlers", () => {
  it("records an uncaught error and an unhandled rejection", () => {
    const { target, dispatch } = fakeWindow();
    const remove = installGlobalHandlers(target);

    dispatch("error", { error: new Error("render failed") });
    dispatch("unhandledrejection", { reason: new Error("fetch failed") });

    const messages = clientEvents().map((event) => event.message);
    expect(messages.some((message) => message.includes("render failed"))).toBe(true);
    expect(messages.some((message) => message.includes("fetch failed"))).toBe(true);
    expect(clientEvents().map((event) => event.source)).toEqual(["window", "promise"]);

    remove();
    dispatch("error", { error: new Error("after removal") });
    expect(clientEvents()).toHaveLength(2);
  });

  it("records a syntax error that arrives with only a message", () => {
    const { target, dispatch } = fakeWindow();
    installGlobalHandlers(target);
    dispatch("error", { message: "Unexpected token" });
    expect(clientEvents()[0]?.message).toBe("Unexpected token");
  });
});
