import type { Attribution, ComponentSource } from "../lib/types";

const SOURCE_ORDER: ComponentSource[] = ["rust", "web", "native"];

const SOURCE_LABEL: Record<ComponentSource, string> = {
  rust: "Rust crates",
  web: "Web packages",
  native: "Bundled native libraries",
};

export type ComponentGroup = {
  source: ComponentSource;
  label: string;
  components: Attribution[];
};

export function sourceLabel(source: ComponentSource): string {
  return SOURCE_LABEL[source];
}

export function matchesQuery(component: Attribution, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  return (
    component.name.toLowerCase().includes(needle) ||
    component.license.toLowerCase().includes(needle) ||
    (component.note ?? "").toLowerCase().includes(needle)
  );
}

export function groupComponents(components: Attribution[], query: string): ComponentGroup[] {
  const matched = components.filter((component) => matchesQuery(component, query));
  return SOURCE_ORDER.map((source) => ({
    source,
    label: SOURCE_LABEL[source],
    components: matched.filter((component) => component.source === source),
  })).filter((group) => group.components.length > 0);
}

export function licenseSummary(components: Attribution[]): { license: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const component of components) {
    counts.set(component.license, (counts.get(component.license) ?? 0) + 1);
  }
  return [...counts]
    .map(([license, count]) => ({ license, count }))
    .toSorted((a, b) => b.count - a.count || a.license.localeCompare(b.license));
}

export function notedComponents(components: Attribution[]): Attribution[] {
  return components.filter((component) => component.note !== undefined && component.note !== null);
}
