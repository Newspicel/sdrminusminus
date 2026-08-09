// Shared-token storage (PLAN §12: "UI prompts and stores it per saved connection"). The token
// is the only thing the browser persists besides UI preferences (PLAN §11), and it is scoped
// to the origin it was entered for so a saved token never leaks to another server.
//
// Kept out of TanStack Query and out of React state on purpose: `api.ts`'s fetch middleware
// and the WebSocket URL both need it synchronously, before any component has rendered.

const KEY = "sdrmm.v1.token";

let cached: string | null = null;
let loaded = false;

/** Storage can throw (private mode, embedded webviews) or be absent entirely (the test
 * environment, a future SSR pass); a token is never worth a crash. */
function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function getToken(): string | null {
  if (!loaded) {
    cached = storage()?.getItem(KEY) ?? null;
    loaded = true;
  }
  return cached;
}

export function setToken(token: string | null): void {
  cached = token && token.length > 0 ? token : null;
  loaded = true;
  const store = storage();
  if (!store) {
    return;
  }
  try {
    if (cached === null) {
      store.removeItem(KEY);
    } else {
      store.setItem(KEY, cached);
    }
  } catch {
    // A full or blocked store just means the token lives for this session only.
  }
}

/** Append the token to a URL the browser fetches itself — a WebSocket handshake or a download
 * navigation, neither of which can carry an `Authorization` header. */
export function withToken(url: string): string {
  const token = getToken();
  if (token === null) {
    return url;
  }
  const separator = url.includes("?") ? "&" : "?";
  return `${url}${separator}token=${encodeURIComponent(token)}`;
}

/** Test seam: forget the in-memory copy so a test can change what storage returns. */
export function resetTokenCache(): void {
  cached = null;
  loaded = false;
}
