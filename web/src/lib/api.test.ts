// Regression for the dead-backend path: the dev proxy answers 502 with an empty (or HTML)
// body while the server is down, which openapi-fetch surfaces as `{ error: undefined }` /
// a plain string — previously read as success, crashing on `data.id` and reporting deletes
// as applied.
import { describe, expect, it } from "vitest";
import { unwrap } from "./api";

describe("unwrap", () => {
  it("returns the data of an ok response", () => {
    const result = { data: { id: 3 }, response: new Response(null, { status: 200 }) };
    expect(unwrap(result)).toEqual({ id: 3 });
  });

  it("passes a bodyless 204 through for void endpoints", () => {
    const result = { data: undefined, response: new Response(null, { status: 204 }) };
    expect(unwrap(result)).toBeUndefined();
  });

  it("formats an ApiError body with and without detail", () => {
    const response = new Response(null, { status: 400 });
    expect(() => unwrap({ error: { error: "invalid", detail: "offset" }, response })).toThrow(
      "invalid: offset",
    );
    expect(() => unwrap({ error: { error: "invalid" }, response })).toThrow(/^invalid$/);
  });

  it("throws on an error response with an empty body instead of faking success", () => {
    const result = { error: undefined, response: new Response(null, { status: 502 }) };
    expect(() => unwrap(result)).toThrow("HTTP 502: no response from the server");
  });

  it("surfaces a non-JSON error body as text", () => {
    const result = { error: "Bad Gateway", response: new Response(null, { status: 502 }) };
    expect(() => unwrap(result)).toThrow("HTTP 502: Bad Gateway");
  });
});
