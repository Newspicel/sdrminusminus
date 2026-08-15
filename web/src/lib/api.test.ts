// Regression for the dead-backend path: the dev proxy answers 502 with an empty (or HTML)
// body while the server is down, which openapi-fetch surfaces as `{ error: undefined }` /
// a plain string — previously read as success, crashing on `data.id` and reporting deletes
// as applied.
import { keepPreviousData } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  client,
  decoderLogExportUrl,
  NMEA_DEVICES_KEY,
  nmeaDevicesQuery,
  recordingDownloadUrl,
  TOOL_RUN_KEY,
  toolRunQuery,
  unwrap,
} from "./api";
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

describe("nmeaDevicesQuery", () => {
  afterEach(() => vi.restoreAllMocks());

  it("fetches and unwraps the briefly cached serial-device list", async () => {
    const response = { devices: [{ path: "/dev/ttyUSB0" }] };
    const get = vi.spyOn(client, "GET").mockResolvedValue({
      data: response,
      response: new Response(null, { status: 200 }),
    });
    const query = nmeaDevicesQuery();

    expect(query.queryKey).toBe(NMEA_DEVICES_KEY);
    expect(query.staleTime).toBe(5_000);
    await expect(query.queryFn?.({} as never)).resolves.toEqual(response);
    expect(get).toHaveBeenCalledOnce();
    expect(get).toHaveBeenCalledWith("/api/position/nmea-devices");
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
    expect(decoderLogExportUrl("csv", { nodes: "", sources: "" })).toBe(
      "/api/decoderlog/export/csv?nodes=&sources=",
    );
    expect(decoderLogExportUrl("json", { nodes: "channel:a1", sources: "0:1,0:2" })).toBe(
      "/api/decoderlog/export/json?nodes=channel%3Aa1&sources=0%3A1%2C0%3A2",
    );
  });

  it("drops the row limit the export endpoint ignores", () => {
    expect(decoderLogExportUrl("csv", { limit: 500, sources: "0:1" })).toBe(
      "/api/decoderlog/export/csv?sources=0%3A1",
    );
  });
});

describe("toolRunQuery", () => {
  it("keeps the previous answer on screen while the next arguments are worked out", () => {
    const query = toolRunQuery({ tool: "antenna", request: { frequency_hz: 1e6 } } as never);

    expect(query.placeholderData).toBe(keepPreviousData);
    expect(query.enabled).toBe(true);
    expect(query.queryKey).toEqual([
      ...TOOL_RUN_KEY,
      { tool: "antenna", request: { frequency_hz: 1e6 } },
    ]);
  });

  it("stays idle without a request", () => {
    expect(toolRunQuery(null).enabled).toBe(false);
  });
});
