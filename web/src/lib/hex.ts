const UTF8 = new TextEncoder();

export function hexUtf8(text: string): string {
  return Array.from(UTF8.encode(text), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
