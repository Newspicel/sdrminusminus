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

export function pickNodes<T>(
  record: Readonly<Record<string, T>>,
  nodes: readonly string[],
): Readonly<Record<string, T>> {
  const picked: Record<string, T> = {};
  for (const node of nodes) {
    const value = record[node];
    if (value !== undefined) {
      picked[node] = value;
    }
  }
  return picked;
}
