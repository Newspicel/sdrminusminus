import { useRef, useState } from "react";
import { Button } from "../components/BaseControls";
import { formatHz } from "../components/format";
import { SearchField } from "../components/ListPanel";
import { Tip } from "../components/Tip";
import type { NodeKind } from "../lib/types";
import { useWorkspaceContext } from "./context";
import {
  filterPalette,
  firstPaletteItem,
  type PaletteGroup,
  type PaletteItem,
  type PaletteSection,
  paletteGroups,
  SECTIONS,
  sectionGroups,
} from "./palette";

const SECTION_KEY = "sdrmm.palette.section";

export function NodePalette({ onAdd }: { onAdd: (kind: NodeKind, channelType?: string) => void }) {
  const workspace = useWorkspaceContext();
  const [query, setQuery] = useState("");
  const [section, setSection] = useState<PaletteSection>(storedSection);
  const listRef = useRef<HTMLDivElement>(null);
  const all = paletteGroups(workspace.context.catalog, workspace.context.channelTypes);
  const found = filterPalette(all, query);
  const searching = query.trim() !== "";
  const shown = searching ? found : sectionGroups(all, section);
  const pick = (item: PaletteItem | undefined) => {
    if (item !== undefined) {
      onAdd(item.kind, item.type?.type_id);
    }
  };
  const choose = (next: PaletteSection) => {
    setSection(next);
    setQuery("");
    storeSection(next);
  };

  return (
    <div className="flex h-[min(32rem,72vh)]">
      <nav className="flex w-36 shrink-0 flex-col gap-px border-r border-line bg-panel p-1.5">
        {SECTIONS.map((entry) => (
          <RailEntry
            key={entry.id}
            section={entry.id}
            title={entry.title}
            count={countItems(sectionGroups(searching ? found : all, entry.id))}
            active={!searching && section === entry.id}
            onClick={() => choose(entry.id)}
          />
        ))}
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="shrink-0 border-b border-line p-2">
          <SearchField
            autoFocus
            placeholder="search name or purpose"
            aria-label="Search nodes"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                pick(firstPaletteItem(shown));
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                entries(listRef.current)[0]?.focus();
              }
            }}
          />
        </div>
        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto p-2">
          <PaletteList groups={shown} columns={2} onPick={pick} />
        </div>
      </div>
    </div>
  );
}

const SECTION_DOT: Record<PaletteSection, string> = {
  source: "bg-cat-source",
  channel: "bg-cat-channel",
  tool: "bg-cat-tool",
  output: "bg-cat-output",
};

function RailEntry({
  section,
  title,
  count,
  active,
  onClick,
}: {
  section: PaletteSection;
  title: string;
  count: number;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <Button
      type="button"
      aria-pressed={active}
      disabled={count === 0}
      className={`flex h-7 items-center gap-2 rounded-[3px] px-2 text-left font-mono text-[11px] transition-colors duration-100 disabled:opacity-40 ${
        active ? "bg-accent/15 text-accent" : "text-ink-dim hover:bg-panel-2 hover:text-ink"
      }`}
      onClick={onClick}
    >
      <span aria-hidden className={`size-1.5 shrink-0 rounded-full ${SECTION_DOT[section]}`} />
      <span className="flex-1">{title}</span>
      <span className="text-[10px] tabular-nums text-ink-faint">{count}</span>
    </Button>
  );
}

export function PaletteList({
  groups,
  columns,
  onPick,
}: {
  groups: readonly PaletteGroup[];
  columns: 2 | 3;
  onPick: (item: PaletteItem) => void;
}) {
  if (groups.length === 0) {
    return (
      <p className="py-10 text-center text-sm text-ink-faint">
        Nothing matches. Try a mode or a task.
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-3" onKeyDown={moveFocus}>
      {groups.map((group) => (
        <section key={group.id} className="flex flex-col gap-1">
          <span className="border-b border-line px-2 pb-1 font-mono text-[10.5px] text-ink-faint">
            {group.title}
          </span>
          <div className={`grid gap-px ${columns === 3 ? "grid-cols-3" : "grid-cols-2"}`}>
            {group.items.map((item) => (
              <PaletteEntry key={item.id} item={item} onAdd={() => onPick(item)} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

export function PaletteEntry({ item, onAdd }: { item: PaletteItem; onAdd: () => void }) {
  const entry = {
    type: "button",
    "data-palette-entry": "",
    className:
      "group flex min-w-0 flex-col gap-px rounded-[3px] border border-transparent px-2 py-1 text-left outline-none transition-colors duration-100 hover:bg-panel-2 focus-visible:border-accent-dim focus-visible:bg-panel-2",
    onClick: onAdd,
  } as const;
  const face = (
    <>
      <span className="flex w-full items-baseline justify-between gap-2">
        <span className="truncate font-mono text-xs text-ink group-hover:text-accent">
          {item.name}
        </span>
        {item.type !== undefined && (
          <span aria-hidden className="shrink-0 font-mono text-[10px] tabular-nums text-ink-faint">
            {formatHz(item.type.bandwidth_hz)}
          </span>
        )}
      </span>
      {item.summary !== "" && (
        <span
          aria-hidden
          className="w-full truncate text-[11px] text-ink-faint group-hover:text-ink-dim"
        >
          {item.summary}
        </span>
      )}
    </>
  );
  if (item.summary === "") {
    return <Button {...entry}>{face}</Button>;
  }
  return (
    <Tip text={item.summary} render={<Button {...entry} />}>
      {face}
    </Tip>
  );
}

function moveFocus(event: React.KeyboardEvent<HTMLElement>) {
  const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[event.key];
  if (step === undefined) {
    return;
  }
  const all = entries(event.currentTarget);
  const at = all.indexOf(document.activeElement as HTMLElement);
  if (at === -1) {
    return;
  }
  event.preventDefault();
  all[Math.min(Math.max(at + step, 0), all.length - 1)]?.focus();
}

function entries(root: HTMLElement | null): HTMLElement[] {
  return root === null ? [] : [...root.querySelectorAll<HTMLElement>("[data-palette-entry]")];
}

function countItems(groups: readonly PaletteGroup[]): number {
  return groups.reduce((sum, group) => sum + group.items.length, 0);
}

function storedSection(): PaletteSection {
  try {
    const stored = localStorage.getItem(SECTION_KEY);
    return SECTIONS.find((entry) => entry.id === stored)?.id ?? "source";
  } catch {
    return "source";
  }
}

function storeSection(section: PaletteSection) {
  try {
    localStorage.setItem(SECTION_KEY, section);
  } catch {
    return;
  }
}
