import { describe, expect, it } from "vitest";
import { serverDownDetail } from "./ServerDown";

describe("serverDownDetail", () => {
  it("drops what the transport says when a connection is simply refused", () => {
    expect(serverDownDetail("Failed to fetch")).toBeNull();
    expect(serverDownDetail("Load failed")).toBeNull();
    expect(serverDownDetail("NetworkError when attempting to fetch resource.")).toBeNull();
    expect(serverDownDetail("TypeError: Failed to fetch")).toBeNull();
  });

  it("drops the dev proxy's empty 500, which is the same refusal one hop later", () => {
    expect(serverDownDetail("HTTP 500: no response from the server")).toBeNull();
  });

  it("keeps a reason the server itself gave", () => {
    expect(serverDownDetail("HTTP 503: starting up")).toBe("HTTP 503: starting up");
    expect(serverDownDetail("database is locked")).toBe("database is locked");
  });

  it("treats blank and missing alike", () => {
    expect(serverDownDetail(null)).toBeNull();
    expect(serverDownDetail("   ")).toBeNull();
  });
});
