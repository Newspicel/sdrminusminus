import { FIELD_ROOT } from "../field/missions";
import { getToken, TOKEN_PARAM } from "../lib/auth";

/// Every address the phone could be pointed at, best first.
///
/// The operator's own origin is useless when it is `localhost`: that name means the phone, not the
/// server. The addresses the server reports for itself are the ones a second device can follow, so
/// they come first whenever the browser is on a loopback origin.
export function handoffOrigins(origin: string, lanAddresses: readonly string[]): string[] {
  const parsed = safeUrl(origin);
  const local = parsed !== null && isLoopback(parsed.hostname);
  const fromLan = lanAddresses.map((address) =>
    parsed === null
      ? `http://${address}`
      : `${parsed.protocol}//${wrap(address)}${parsed.port === "" ? "" : `:${parsed.port}`}`,
  );
  const own = parsed === null ? [] : [parsed.origin];
  const ordered = local ? [...fromLan, ...own] : [...own, ...fromLan];
  return [...new Set(ordered)];
}

/// The URL a phone should open: the field route, carrying the token when one is needed.
export function handoffUrl(origin: string, token: string | null = getToken()): string {
  const url = new URL(FIELD_ROOT, origin.endsWith("/") ? origin : `${origin}/`);
  if (token !== null && token.length > 0) {
    url.searchParams.set(TOKEN_PARAM, token);
  }
  return url.toString();
}

function safeUrl(origin: string): URL | null {
  try {
    return new URL(origin);
  } catch {
    return null;
  }
}

function isLoopback(hostname: string): boolean {
  return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "[::1]";
}

function wrap(address: string): string {
  return address.includes(":") ? `[${address}]` : address;
}
