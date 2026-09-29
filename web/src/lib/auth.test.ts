import { beforeEach, describe, expect, it } from "vitest";
import {
  adoptTokenFromUrl,
  getToken,
  loginRedirect,
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

function fakeHistory(): { history: History; seen: string[] } {
  const seen: string[] = [];
  const history = {
    replaceState: (_state: unknown, _title: string, url?: string | URL | null) => {
      seen.push(String(url ?? ""));
    },
  } as unknown as History;
  return { history, seen };
}

describe("adoptTokenFromUrl", () => {
  it("keeps a token from the address bar and takes it back out of the address", () => {
    resetTokenCache();
    setToken(null);
    const { history, seen } = fakeHistory();
    const location = new URL("http://192.168.1.10:8080/field?token=s3cret&mission=df");
    const adopted = adoptTokenFromUrl(location as unknown as Location, history);
    expect(adopted).toBe("s3cret");
    expect(getToken()).toBe("s3cret");
    expect(seen).toEqual(["/field?mission=df"]);
  });

  it("leaves an address without a token alone", () => {
    resetTokenCache();
    setToken(null);
    const { history, seen } = fakeHistory();
    const location = new URL("http://localhost:8080/field");
    expect(adoptTokenFromUrl(location as unknown as Location, history)).toBeNull();
    expect(seen).toEqual([]);
  });
});

describe("loginRedirect", () => {
  const login = "https://app.sdrmm.com/open/abc";

  it("sends a tokenless visitor to the app that can grant a token", () => {
    expect(loginRedirect({ token_required: true, login_url: login }, false)).toBe(login);
  });

  it("stays put with a token, without a login URL, or when no token is needed", () => {
    expect(loginRedirect({ token_required: true, login_url: login }, true)).toBeNull();
    expect(loginRedirect({ token_required: true }, false)).toBeNull();
    expect(loginRedirect({ token_required: false, login_url: login }, false)).toBeNull();
    expect(loginRedirect(undefined, false)).toBeNull();
  });
});
