// What an unreachable server is worth saying out loud. Kept out of `ServerDown.tsx` so that file
// exports only components — a mixed module costs Fast Refresh the component state it would
// otherwise preserve.

/** Failures that say nothing the headline has not already said. A browser reports a refused
 * connection as "Failed to fetch" (Safari: "Load failed"), and the dev proxy turns the same
 * refusal into an empty 500 — repeating any of those under "can't reach the server" is noise
 * dressed as a diagnosis. Anything else is the server's own words and worth showing. */
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
