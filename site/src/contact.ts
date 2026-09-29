export const CONTACT_PATH = "/api/contact";
export const CONTACT_PAGE = "/business.html";

export const FIELDS = {
  name: "name",
  email: "email",
  message: "message",
  trap: "website",
} as const;

export const LIMITS = {
  name: 200,
  email: 254,
  message: 5000,
} as const;

export const OUTCOMES = ["sent", "invalid", "busy", "failed"] as const;

export type ContactOutcome = (typeof OUTCOMES)[number];

export interface ContactMessage {
  name: string;
  email: string;
  message: string;
}

export type ParsedContact =
  | { kind: "message"; message: ContactMessage }
  | { kind: "invalid" }
  | { kind: "trapped" };

const EMAIL = /^[^\s@<>"(),;:]+@[^\s@<>"(),;:]+\.[^\s@<>"(),;:]+$/;
const CONTROL = /\p{Cc}/u;

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function singleLine(value: string, limit: number): boolean {
  return value.length > 0 && value.length <= limit && !CONTROL.test(value);
}

export function parseContact(field: (name: string) => unknown): ParsedContact {
  if (text(field(FIELDS.trap)) !== "") {
    return { kind: "trapped" };
  }
  const name = text(field(FIELDS.name));
  const email = text(field(FIELDS.email));
  const message = text(field(FIELDS.message));
  const valid =
    singleLine(name, LIMITS.name) &&
    singleLine(email, LIMITS.email) &&
    EMAIL.test(email) &&
    message.length > 0 &&
    message.length <= LIMITS.message;
  return valid ? { kind: "message", message: { name, email, message } } : { kind: "invalid" };
}

export function isOutcome(value: unknown): value is ContactOutcome {
  return OUTCOMES.some((outcome) => outcome === value);
}
