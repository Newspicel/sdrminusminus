import { beforeEach, describe, expect, it } from "vitest";
import {
  getToken,
  onTokenRejected,
  rejectToken,
  resetTokenCache,
  setToken,
  withToken,
} from "./auth";

class FakeStorage {
  private readonly items = new Map<string, string>();
  get length(): number {
    return this.items.size;
  }
  key(index: number): string | null {
    return [...this.items.keys()][index] ?? null;
  }
  getItem(key: string): string | null {
    return this.items.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.items.set(key, value);
  }
  removeItem(key: string): void {
    this.items.delete(key);
  }
  clear(): void {
    this.items.clear();
  }
}

beforeEach(() => {
  Object.defineProperty(globalThis, "localStorage", {
    value: new FakeStorage(),
    configurable: true,
  });
  resetTokenCache();
});

describe("token storage", () => {
  it("round-trips through localStorage and forgets on clear", () => {
    expect(getToken()).toBeNull();
    setToken("s3cret");
    expect(getToken()).toBe("s3cret");
    resetTokenCache();
    expect(getToken()).toBe("s3cret");
    setToken(null);
    expect(getToken()).toBeNull();
  });

  it("treats an empty token as no token", () => {
    setToken("");
    expect(getToken()).toBeNull();
  });
});

describe("withToken", () => {
  it("leaves URLs alone when no token is stored", () => {
    expect(withToken("/api/ws")).toBe("/api/ws");
  });

  it("appends with the right separator and escapes the value", () => {
    setToken("a/b c");
    expect(withToken("/api/ws")).toBe("/api/ws?token=a%2Fb%20c");
    expect(withToken("/api/decoderlog/export/csv?kind=adsb")).toBe(
      "/api/decoderlog/export/csv?kind=adsb&token=a%2Fb%20c",
    );
  });
});

describe("rejectToken", () => {
  it("forgets a refused token and tells the gate exactly once", () => {
    let notified = 0;
    const stop = onTokenRejected(() => {
      notified += 1;
    });
    setToken("wrong");
    rejectToken();
    expect(getToken()).toBeNull();
    expect(notified).toBe(1);

    rejectToken();
    expect(notified).toBe(1);
    stop();
  });
});
