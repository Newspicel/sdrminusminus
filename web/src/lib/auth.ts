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

/** Listeners notified when the server rejects the stored token. */
const rejected = new Set<() => void>();

/** Subscribe to "the stored token was refused"; returns the unsubscribe. */
export function onTokenRejected(listener: () => void): () => void {
  rejected.add(listener);
  return () => {
    rejected.delete(listener);
  };
}

/** The server answered 401 while a token was stored: forget it and let the gate ask again.
 * Without this a wrong or stale token is retried forever and the UI never recovers — the
 * browser cannot tell a rejected WebSocket handshake from an outage, so nothing else would. */
export function rejectToken(): void {
  if (getToken() === null) {
    return;
  }
  setToken(null);
  for (const listener of rejected) {
    listener();
  }
}

/** Test seam: forget the in-memory copy so a test can change what storage returns. */
export function resetTokenCache(): void {
  cached = null;
  loaded = false;
}
