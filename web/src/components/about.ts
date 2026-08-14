// Shaping for the About panel. Pure, so the searching and grouping are tested without a DOM.
//
// The panel exists because MIT and BSD require the copyright notice to travel with the binary:
// the notices are part of what is shipped, not documentation about it. That is also why the
// search matches licenses as well as names — the question a reader actually arrives with is
// "is there any copyleft in here", not "which version of `serde` is this".
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

/** Name, license and note all match: a reader looking for "GPL" wants the components whose
 * license says so, and the note is where the reason a license matters is written down. */
export function matchesQuery(component: Attribution, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  return (
    component.name.toLowerCase().includes(needle) ||
    component.license.toLowerCase().includes(needle) ||
    (component.note ?? "").toLowerCase().includes(needle)
  );
}

/** Groups in a fixed order, and empty groups dropped so a search never leaves a bare heading
 * with nothing under it. */
export function groupComponents(components: Attribution[], query: string): ComponentGroup[] {
  const matched = components.filter((component) => matchesQuery(component, query));
  return SOURCE_ORDER.map((source) => ({
    source,
    label: SOURCE_LABEL[source],
    components: matched.filter((component) => component.source === source),
  })).filter((group) => group.components.length > 0);
}

/** Distinct license expressions, most common first, for the summary line. Ties break by name so
 * the order is stable across builds rather than dependent on harvest order. */
export function licenseSummary(components: Attribution[]): { license: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const component of components) {
    counts.set(component.license, (counts.get(component.license) ?? 0) + 1);
  }
  return [...counts]
    .map(([license, count]) => ({ license, count }))
    .sort((a, b) => b.count - a.count || a.license.localeCompare(b.license));
}

/** The components whose license needs explaining. These are the reason the panel is worth
 * opening, so they are lifted out of the alphabetical bulk. */
export function notedComponents(components: Attribution[]): Attribution[] {
  return components.filter((component) => component.note !== undefined && component.note !== null);
}
