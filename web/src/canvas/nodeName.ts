export function nodeLabel(typed: string, title: string): string | undefined {
  const trimmed = typed.trim();
  return trimmed === "" || trimmed === title ? undefined : trimmed;
}
