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
