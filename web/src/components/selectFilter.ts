export interface Choice<T extends string | number> {
  value: T;
  label: string;
}

function normalize(text: string): string {
  return text.toLowerCase().replaceAll(/[^a-z0-9]/g, "");
}

export function optionMatches<T extends string | number>(item: Choice<T>, query: string): boolean {
  const needle = normalize(query);
  if (needle === "") {
    return true;
  }
  return normalize(item.label).includes(needle) || normalize(String(item.value)).includes(needle);
}
