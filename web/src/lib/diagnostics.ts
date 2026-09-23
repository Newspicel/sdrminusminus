import type { LogLevel } from "./types";

export interface ClientEvent {
  at: string;
  level: LogLevel;
  source: string;
  message: string;
}

const CAPACITY = 200;
const MAX_MESSAGE_LEN = 2000;

const ring: ClientEvent[] = [];
let dropped = 0;

export function recordEvent(level: LogLevel, source: string, message: string): void {
  while (ring.length >= CAPACITY) {
    ring.shift();
    dropped += 1;
  }
  ring.push({
    at: new Date().toISOString(),
    level,
    source,
    message: truncate(redactText(message), MAX_MESSAGE_LEN),
  });
}

export function clientEvents(): readonly ClientEvent[] {
  return ring;
}

export function droppedEvents(): number {
  return dropped;
}

export function resetEvents(): void {
  ring.length = 0;
  dropped = 0;
}

export function describeError(value: unknown): string {
  if (value instanceof Error) {
    return value.stack !== undefined && value.stack.length > 0
      ? value.stack
      : `${value.name}: ${value.message}`;
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

export interface ErrorSource {
  addEventListener(type: string, handler: (event: Event) => void): void;
  removeEventListener(type: string, handler: (event: Event) => void): void;
}

const onError = (event: Event) => {
  const raised = event as Event & { error?: unknown; message?: string };
  recordEvent("error", "window", describeError(raised.error ?? raised.message));
};

const onRejection = (event: Event) => {
  recordEvent("error", "promise", describeError((event as Event & { reason?: unknown }).reason));
};

export function installGlobalHandlers(target: ErrorSource): () => void {
  target.addEventListener("error", onError);
  target.addEventListener("unhandledrejection", onRejection);
  return () => {
    target.removeEventListener("error", onError);
    target.removeEventListener("unhandledrejection", onRejection);
  };
}

const TOKEN_QUERY = /([?&](?:token|access_token|auth)=)[^&\s"']+/gi;
const BEARER = /\b(Bearer\s+)[\w\-._~+/]+=*/gi;
const IPV4 = /\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/g;

export function redactText(text: string): string {
  return text
    .replace(TOKEN_QUERY, "$1<redacted>")
    .replace(BEARER, "$1<redacted>")
    .replace(IPV4, (match) => (identifyingIpv4(match) ? "<ip>" : match));
}

function identifyingIpv4(address: string): boolean {
  const octets = address.split(".").map(Number);
  if (octets.length !== 4 || octets.some((part) => !Number.isInteger(part) || part > 255)) {
    return false;
  }
  if (octets[0] === 127 || octets.every((part) => part === 0)) {
    return false;
  }
  return true;
}

function truncate(text: string, limit: number): string {
  return text.length <= limit ? text : `${text.slice(0, limit)}…`;
}
