export function omitNodes<T>(
  record: Readonly<Record<string, T>>,
  nodes: readonly string[],
): Readonly<Record<string, T>> {
  if (!nodes.some((node) => node in record)) {
    return record;
  }
  const kept: Record<string, T> = { ...record };
  for (const node of nodes) {
    delete kept[node];
  }
  return kept;
}
