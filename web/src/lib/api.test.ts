// Regression for the dead-backend path: the dev proxy answers 502 with an empty (or HTML)
// body while the server is down, which openapi-fetch surfaces as `{ error: undefined }` /
// a plain string — previously read as success, crashing on `data.id` and reporting deletes
// as applied.
import { afterEach, describe, expect, it } from "vitest";
import { decoderLogExportUrl, recordingDownloadUrl, unwrap } from "./api";
import { setToken } from "./auth";

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

describe("recordingDownloadUrl", () => {
  afterEach(() => setToken(null));

  it("leaves the server's default format out of the URL", () => {
    expect(recordingDownloadUrl(7, "sigmf")).toBe("/api/recordings/7/download");
  });

  it("names any other container", () => {
    expect(recordingDownloadUrl(7, "wav")).toBe("/api/recordings/7/download?format=wav");
  });

  // The browser navigates to this href, so it cannot carry an Authorization header; against a
  // tokened server the download 401s unless the token rides in the query.
  it("carries the token, joining an existing query correctly", () => {
    setToken("s3cret/token");
    expect(recordingDownloadUrl(7, "sigmf")).toBe(
      "/api/recordings/7/download?token=s3cret%2Ftoken",
    );
    expect(recordingDownloadUrl(7, "wav")).toBe(
      "/api/recordings/7/download?format=wav&token=s3cret%2Ftoken",
    );
  });
});

describe("decoderLogExportUrl", () => {
  afterEach(() => setToken(null));

  it("drops a blank text filter but never a blank wire scope", () => {
    expect(decoderLogExportUrl("csv", { q: "", kind: "adsb" })).toBe(
      "/api/decoderlog/export/csv?kind=adsb",
    );
    // Empty means *no channels*. Dropped, it would mean every channel, and the same filter
    // backs the clear endpoint — so this is the difference between exporting one node's rows
    // and emptying the log.
    expect(decoderLogExportUrl("csv", { sources: "" })).toBe("/api/decoderlog/export/csv?sources=");
    expect(decoderLogExportUrl("json", { sources: "0:1,0:2" })).toBe(
      "/api/decoderlog/export/json?sources=0%3A1%2C0%3A2",
    );
  });

  it("drops the row limit the export endpoint ignores", () => {
    expect(decoderLogExportUrl("csv", { limit: 500, sources: "0:1" })).toBe(
      "/api/decoderlog/export/csv?sources=0%3A1",
    );
  });
});
