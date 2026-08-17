export interface Selectable {
  id: string;
  selected?: boolean;
}

export function focusNode<T extends Selectable>(nodes: T[], focus: string | null): T[] {
  if (focus === null) {
    return nodes;
  }
  const target = nodes.find((node) => node.id === focus);
  if (target === undefined || target.selected === true) {
    return nodes;
  }
  return nodes.map((node) => {
    const wanted = node.id === focus;
    return (node.selected ?? false) === wanted ? node : { ...node, selected: wanted };
  });
}
