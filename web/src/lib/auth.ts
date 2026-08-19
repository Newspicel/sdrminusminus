const KEY = "sdrmm.v1.token";

let cached: string | null = null;
let loaded = false;

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
  } catch {}
}

export function withToken(url: string): string {
  const token = getToken();
  if (token === null) {
    return url;
  }
  const separator = url.includes("?") ? "&" : "?";
  return `${url}${separator}token=${encodeURIComponent(token)}`;
}

const rejected = new Set<() => void>();

export function onTokenRejected(listener: () => void): () => void {
  rejected.add(listener);
  return () => {
    rejected.delete(listener);
  };
}

export function rejectToken(): void {
  if (getToken() === null) {
    return;
  }
  setToken(null);
  for (const listener of rejected) {
    listener();
  }
}

export function resetTokenCache(): void {
  cached = null;
  loaded = false;
}

export const TOKEN_PARAM = "token";

/// Takes a token out of the address bar, keeps it, and puts the address back the way it should
/// have been.
///
/// This is how a phone joins: the operator scans a QR code that carries the token, and the phone
/// must not be left holding a URL that leaks it into history, a screenshot or a shared link.
export function adoptTokenFromUrl(location: Location, history: History): string | null {
  const url = new URL(location.href);
  const token = url.searchParams.get(TOKEN_PARAM);
  if (token === null || token.length === 0) {
    return null;
  }
  setToken(token);
  url.searchParams.delete(TOKEN_PARAM);
  const search = url.searchParams.toString();
  history.replaceState(null, "", `${url.pathname}${search === "" ? "" : `?${search}`}${url.hash}`);
  return token;
}
