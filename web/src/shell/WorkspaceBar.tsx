// Workspace switcher + tab strip (PLAN §10, M6). Workspaces are station config, so both live on
// the server: switching one switches it for every client on the radio.
import { useState } from "react";
import { BTN, FIELD } from "../components/controls";
import type { PanelKind, TabSpec, WorkspaceInfo, WorkspaceSnapshot } from "../lib/types";
import { tabPanels } from "./dockLayout";
import { PANEL_KINDS, panelId, panelTitle } from "./panels";

export function WorkspaceBar({
  workspaces,
  activeId,
  snapshot,
  onSnapshot,
  onActivate,
  onCreate,
  onRemove,
}: {
  workspaces: readonly WorkspaceInfo[];
  activeId: number | null;
  snapshot: WorkspaceSnapshot | null;
  onSnapshot: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRemove: (id: number) => void;
}) {
  const [newName, setNewName] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const tabs = snapshot?.tabs ?? [];
  const activeTab = tabs.find((tab) => tab.id === snapshot?.active_tab) ?? tabs[0] ?? null;

  const write = (edit: (current: WorkspaceSnapshot) => Partial<WorkspaceSnapshot>) => {
    onSnapshot((current) => ({ ...current, ...edit(current) }));
  };

  const replaceTab = (tab: TabSpec) => {
    write((current) => ({
      tabs: current.tabs.map((existing) => (existing.id === tab.id ? tab : existing)),
    }));
  };

  const addTab = () => {
    // Ids are generated, never derived from the name: renaming a tab must not orphan the
    // `active_tab` pointer or a template's `template:<id>` tab.
    const id = `tab-${Date.now().toString(36)}`;
    const tab: TabSpec = {
      id,
      name: `Tab ${tabs.length + 1}`,
      layout: { node: "group", data: { panels: [{ id: panelId("spectrum"), kind: "spectrum" }] } },
    };
    write((current) => ({ tabs: [...current.tabs, tab], active_tab: id }));
  };

  const closeTab = (id: string) => {
    if (tabs.length < 2) {
      return;
    }
    write((current) => {
      const remaining = current.tabs.filter((tab) => tab.id !== id);
      return {
        tabs: remaining.length > 0 ? remaining : current.tabs,
        active_tab: current.active_tab === id ? remaining[0]?.id : current.active_tab,
      };
    });
  };

  const addPanel = (kind: PanelKind) => {
    if (activeTab === undefined || activeTab === null) {
      return;
    }
    const panel = { id: panelId(kind), kind };
    // Into the first group, as its active panel: a new panel the user cannot see reads as the
    // control having done nothing.
    replaceTab({ ...activeTab, layout: insertPanel(activeTab.layout, panel) });
  };

  const missing = activeTab
    ? PANEL_KINDS.filter((kind) => !tabPanels(activeTab).some((panel) => panel.kind === kind))
    : [];

  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-line bg-panel px-4 py-1.5">
      <select
        className={FIELD}
        value={activeId ?? ""}
        onChange={(e) => onActivate(Number(e.target.value))}
        aria-label="Workspace"
      >
        {workspaces.map((workspace) => (
          <option key={workspace.id} value={workspace.id}>
            {workspace.name}
          </option>
        ))}
      </select>
      <form
        className="flex items-center gap-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (newName.trim() !== "") {
            onCreate(newName.trim());
            setNewName("");
          }
        }}
      >
        <input
          className={`${FIELD} w-32`}
          value={newName}
          placeholder="new workspace"
          onChange={(e) => setNewName(e.target.value)}
          aria-label="New workspace name"
        />
        <button type="submit" className={BTN} disabled={newName.trim() === ""}>
          add
        </button>
      </form>
      <button
        type="button"
        className={BTN}
        disabled={activeId === null || workspaces.length < 2}
        onClick={() => activeId !== null && onRemove(activeId)}
        title={workspaces.length < 2 ? "The last workspace cannot be removed" : undefined}
      >
        remove
      </button>

      <span className="h-4 w-px bg-line max-md:hidden" />

      {/* On a phone the tab strip gets its own row: sharing one with the workspace controls
          pushes the tabs off the edge, which is where they are least findable. */}
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1 max-md:basis-full">
        {tabs.map((tab) => {
          const active = tab.id === activeTab?.id;
          if (active && renaming === tab.id) {
            return (
              <input
                // The field replaces the tab button the user just clicked; without focus the
                // rename would need a second click on a control that was not there before.
                autoFocus
                key={tab.id}
                className={`${FIELD} w-28`}
                defaultValue={tab.name}
                aria-label="Tab name"
                onBlur={(e) => {
                  setRenaming(null);
                  const name = e.target.value.trim();
                  if (name !== "" && name !== tab.name) {
                    replaceTab({ ...tab, name });
                  }
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === "Escape") {
                    e.currentTarget.blur();
                  }
                }}
              />
            );
          }
          return (
            <span key={tab.id} className="flex items-center">
              <button
                type="button"
                className={`min-h-8 rounded-l border border-line px-2 font-mono text-xs max-md:min-h-10 ${
                  active ? "border-accent bg-panel-2 text-accent" : "text-ink-dim"
                }`}
                onClick={() =>
                  active ? setRenaming(tab.id) : write(() => ({ active_tab: tab.id }))
                }
                title={active ? "Click again to rename" : undefined}
              >
                {tab.name}
              </button>
              <button
                type="button"
                className="min-h-8 rounded-r border border-line border-l-0 px-1.5 font-mono text-xs text-ink-dim max-md:min-h-10"
                onClick={() => closeTab(tab.id)}
                disabled={tabs.length < 2}
                aria-label={`Close ${tab.name}`}
              >
                ×
              </button>
            </span>
          );
        })}
        <button type="button" className={BTN} onClick={addTab} aria-label="Add tab">
          + tab
        </button>
      </div>

      <select
        className={FIELD}
        value=""
        disabled={missing.length === 0}
        onChange={(e) => addPanel(e.target.value as PanelKind)}
        aria-label="Add panel"
      >
        <option value="" disabled>
          + panel
        </option>
        {missing.map((kind) => (
          <option key={kind} value={kind}>
            {panelTitle(kind)}
          </option>
        ))}
      </select>
    </div>
  );
}

/** Add a panel to the first group of a layout, on top. A panel kind appears at most once per tab
 * (the menu only offers what is missing), so the id stays unique. */
function insertPanel(
  node: TabSpec["layout"],
  panel: { id: string; kind: PanelKind },
): TabSpec["layout"] {
  if (node.node === "group") {
    return { node: "group", data: { panels: [...node.data.panels, panel], active: panel.id } };
  }
  const [first, ...rest] = node.data.children;
  if (first === undefined) {
    return { node: "group", data: { panels: [panel], active: panel.id } };
  }
  return {
    node: "split",
    data: {
      direction: node.data.direction,
      children: [{ ...first, node: insertPanel(first.node, panel) }, ...rest],
    },
  };
}
