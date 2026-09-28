export function step(index: number, key: string, count: number): number | null {
  switch (key) {
    case "ArrowRight":
      return (index + 1) % count;
    case "ArrowLeft":
      return (index - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

export function tabs(
  list: HTMLElement,
  onSelect: (tab: HTMLButtonElement) => void,
): (tab: HTMLButtonElement) => void {
  const all = [...list.querySelectorAll<HTMLButtonElement>("[role=tab]")];
  const select = (tab: HTMLButtonElement) => {
    for (const other of all) {
      other.setAttribute("aria-selected", String(other === tab));
      other.tabIndex = other === tab ? 0 : -1;
    }
    onSelect(tab);
  };
  all.forEach((tab, index) => {
    tab.addEventListener("click", () => select(tab));
    tab.addEventListener("keydown", (event) => {
      const next = all[step(index, event.key, all.length) ?? -1];
      if (next === undefined) {
        return;
      }
      event.preventDefault();
      next.focus();
      select(next);
    });
  });
  return select;
}
