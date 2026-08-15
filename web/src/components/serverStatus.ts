const OPAQUE = [
  /^failed to fetch$/i,
  /^load failed$/i,
  /^networkerror\b/i,
  /^typeerror:/i,
  /no response from the server$/i,
];

export function serverDownDetail(reason: string | null): string | null {
  const trimmed = reason?.trim() ?? "";
  if (trimmed === "" || OPAQUE.some((pattern) => pattern.test(trimmed))) {
    return null;
  }
  return trimmed;
}
