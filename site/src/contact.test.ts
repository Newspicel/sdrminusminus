import { describe, expect, it } from "vitest";
import { FIELDS, isOutcome, LIMITS, parseContact } from "./contact";

function form(values: Partial<Record<keyof typeof FIELDS, string>>) {
  const fields = new Map<string, string>(
    Object.entries(values).map(([key, value]) => [FIELDS[key as keyof typeof FIELDS], value]),
  );
  return (name: string) => fields.get(name) ?? null;
}

const valid = { name: "Ada", email: "ada@example.org", message: "Need a decoder." };

describe("parseContact", () => {
  it("trims a complete message", () => {
    expect(
      parseContact(form({ name: " Ada ", email: "ada@example.org ", message: "\nHi\n" })),
    ).toEqual({
      kind: "message",
      message: { name: "Ada", email: "ada@example.org", message: "Hi" },
    });
  });

  it("keeps line breaks inside the message", () => {
    const parsed = parseContact(form({ ...valid, message: "one\ntwo" }));
    expect(parsed.kind === "message" && parsed.message.message).toBe("one\ntwo");
  });

  it("rejects missing fields", () => {
    expect(parseContact(form({ email: valid.email, message: valid.message })).kind).toBe("invalid");
    expect(parseContact(form({ name: valid.name, message: valid.message })).kind).toBe("invalid");
    expect(parseContact(form({ name: valid.name, email: valid.email })).kind).toBe("invalid");
  });

  it("rejects addresses that are not email addresses", () => {
    for (const email of ["ada", "ada@", "ada@example", "a da@example.org", "<a@b.c>"]) {
      expect(parseContact(form({ ...valid, email })).kind).toBe("invalid");
    }
  });

  it("rejects line breaks in the name, which would reach the subject", () => {
    expect(parseContact(form({ ...valid, name: "Ada\nBcc: x@y.z" })).kind).toBe("invalid");
  });

  it("rejects fields over their limit", () => {
    expect(parseContact(form({ ...valid, name: "a".repeat(LIMITS.name + 1) })).kind).toBe(
      "invalid",
    );
    expect(parseContact(form({ ...valid, message: "a".repeat(LIMITS.message + 1) })).kind).toBe(
      "invalid",
    );
    expect(parseContact(form({ ...valid, message: "a".repeat(LIMITS.message) })).kind).toBe(
      "message",
    );
  });

  it("flags a filled trap field", () => {
    expect(parseContact(form({ ...valid, trap: "https://spam.example" })).kind).toBe("trapped");
  });

  it("ignores values that are not text", () => {
    expect(parseContact((name) => (name === FIELDS.name ? 42 : null)).kind).toBe("invalid");
  });
});

describe("isOutcome", () => {
  it("accepts only known outcomes", () => {
    expect(isOutcome("sent")).toBe(true);
    expect(isOutcome("lost")).toBe(false);
    expect(isOutcome(undefined)).toBe(false);
  });
});
