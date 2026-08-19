import { describe, expect, it } from "vitest";
import { handoffOrigins, handoffUrl } from "./fieldLink";

describe("handoffOrigins", () => {
  it("puts the addresses a phone can follow ahead of localhost", () => {
    expect(handoffOrigins("http://localhost:8080", ["192.168.1.10", "10.0.0.4"])).toEqual([
      "http://192.168.1.10:8080",
      "http://10.0.0.4:8080",
      "http://localhost:8080",
    ]);
  });

  it("keeps the operator's own address first when it already works", () => {
    expect(handoffOrigins("http://192.168.1.10:8080", ["192.168.1.10"])).toEqual([
      "http://192.168.1.10:8080",
    ]);
  });

  it("has only the one address when the server reports none", () => {
    expect(handoffOrigins("http://localhost:8080", [])).toEqual(["http://localhost:8080"]);
  });

  it("keeps the scheme and port of the page it was opened from", () => {
    expect(handoffOrigins("https://localhost:9443", ["192.168.1.10"])[0]).toBe(
      "https://192.168.1.10:9443",
    );
  });
});

describe("handoffUrl", () => {
  it("points at field mode and carries the token when there is one", () => {
    expect(handoffUrl("http://192.168.1.10:8080", null)).toBe("http://192.168.1.10:8080/field");
    expect(handoffUrl("http://192.168.1.10:8080", "s3cret")).toBe(
      "http://192.168.1.10:8080/field?token=s3cret",
    );
  });

  it("escapes a token that needs it", () => {
    expect(handoffUrl("http://host", "a b&c")).toBe("http://host/field?token=a+b%26c");
  });
});
