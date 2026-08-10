// The view bar (DESIGN.md §5): which workspace, which tab, which panels. Everything that
// changes what you are *looking* at, and nothing that changes what the radio is doing.
//
// Workspaces are station config, so both they and their tabs live on the server: switching one
// switches it for every client on the radio.
import { useState } from "react";
import { BTN, BTN_DANGER, BTN_QUIET, FIELD, ICON_BTN, segment } from "../components/controls";
import { Popover } from "../components/Popover";
import { ThemeControl } from "../components/ThemeControl";
import type { PanelKind, TabSpec, WorkspaceInfo, WorkspaceSnapshot } from "../lib/types";
import { tabPanels } from "./dockLayout";
import { PANEL_KINDS, panelId, panelTitle } from "./panels";

export function TabBar({
  workspaces,
  activeId,
  snapshot,
  onSnapshot,
  onActivate,
  onCreate,
  onRemove,
  onShowShortcuts,
}: {
  workspaces: readonly WorkspaceInfo[];
  activeId: number | null;
  snapshot: WorkspaceSnapshot | null;
  onSnapshot: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRemove: (id: number) => void;
  onShowShortcuts: () => void;
}) {
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
    if (activeTab === null) {
      return;
    }
    // Into the first group, as its active panel: a new panel the user cannot see reads as the
    // control having done nothing.
    replaceTab({
      ...activeTab,
      layout: insertPanel(activeTab.layout, { id: panelId(kind), kind }),
    });
  };

  const missing = activeTab
    ? PANEL_KINDS.filter((kind) => !tabPanels(activeTab).some((panel) => panel.kind === kind))
    : [];

  return (
    <div className="flex h-8 shrink-0 items-stretch gap-1 border-b border-line bg-panel pr-1 pl-2">
      <WorkspaceMenu
        workspaces={workspaces}
        activeId={activeId}
        onActivate={onActivate}
        onCreate={onCreate}
        onRemove={onRemove}
      />

      <span aria-hidden className="my-1.5 w-px bg-line" />

      <div className="flex min-w-0 flex-1 items-stretch gap-px overflow-x-auto">
        {tabs.map((tab) => {
          const active = tab.id === activeTab?.id;
          if (active && renaming === tab.id) {
            return (
              <input
                // The field replaces the tab the user just clicked; without focus the rename
                // would need a second click on a control that was not there before.
                autoFocus
                key={tab.id}
                className={`${FIELD} my-0.5 w-28`}
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
            <span key={tab.id} className="group flex items-stretch">
              <button
                type="button"
                aria-current={active ? "page" : undefined}
                className={`px-2.5 text-xs whitespace-nowrap transition-colors duration-100 ${
                  active
                    ? "text-ink shadow-[inset_0_-2px_0_var(--color-accent)]"
                    : "text-ink-dim hover:bg-panel-2 hover:text-ink"
                }`}
                onClick={() => write(() => ({ active_tab: tab.id }))}
                onDoubleClick={() => active && setRenaming(tab.id)}
                title={active ? "Double-click to rename" : undefined}
              >
                {tab.name}
              </button>
              {tabs.length > 1 && (
                <button
                  type="button"
                  // Visible on the tab it belongs to and on hover: an action that only appears
                  // on hover is a tunnel the pointer has to stay inside.
                  className={`px-1 text-xs text-ink-faint hover:text-danger ${
                    active ? "" : "opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                  }`}
                  onClick={() => closeTab(tab.id)}
                  aria-label={`Close ${tab.name}`}
                >
                  ×
                </button>
              )}
            </span>
          );
        })}
        <button
          type="button"
          className={`${ICON_BTN} my-0.5`}
          onClick={addTab}
          aria-label="Add tab"
        >
          +
        </button>
      </div>

      <div className="flex items-center gap-1">
        <Popover
          align="end"
          width="w-44"
          triggerClass={`${BTN_QUIET} my-0.5`}
          label={<span className="legend">Add panel</span>}
        >
          {(close) =>
            missing.length === 0 ? (
              <p className="text-xs text-ink-dim">Every panel is already in this tab.</p>
            ) : (
              <div className="flex flex-col gap-0.5">
                {missing.map((kind) => (
                  <button
                    key={kind}
                    type="button"
                    className={`${segment(false)} justify-start`}
                    onClick={() => {
                      addPanel(kind);
                      close();
                    }}
                  >
                    {panelTitle(kind)}
                  </button>
                ))}
              </div>
            )
          }
        </Popover>
        <ThemeControl />
        <button
          type="button"
          className={`${ICON_BTN} my-0.5`}
          onClick={onShowShortcuts}
          aria-label="Keyboard shortcuts"
          title="Keyboard shortcuts (?)"
        >
          ?
        </button>
      </div>
    </div>
  );
}

function WorkspaceMenu({
  workspaces,
  activeId,
  onActivate,
  onCreate,
  onRemove,
}: {
  workspaces: readonly WorkspaceInfo[];
  activeId: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRemove: (id: number) => void;
}) {
  const [name, setName] = useState("");
  const active = workspaces.find((workspace) => workspace.id === activeId) ?? null;
  return (
    <Popover
      width="w-64"
      triggerClass={`${BTN_QUIET} my-0.5 max-w-40`}
      label={
        <>
          <span className="truncate text-ink">{active?.name ?? "Workspace"}</span>
          <span aria-hidden className="text-ink-faint">
            ▾
          </span>
        </>
      }
    >
      {(close) => (
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-0.5">
            <span className="legend">Workspaces</span>
            {workspaces.map((workspace) => (
              <button
                key={workspace.id}
                type="button"
                className={`${segment(workspace.id === activeId)} justify-start`}
                onClick={() => {
                  onActivate(workspace.id);
                  close();
                }}
              >
                {workspace.name}
              </button>
            ))}
          </div>

          <form
            className="flex items-center gap-1 border-t border-line pt-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (name.trim() !== "") {
                onCreate(name.trim());
                setName("");
                close();
              }
            }}
          >
            <input
              className={`${FIELD} flex-1`}
              value={name}
              placeholder="New workspace"
              aria-label="New workspace name"
              onChange={(event) => setName(event.target.value)}
            />
            <button type="submit" className={BTN} disabled={name.trim() === ""}>
              Add
            </button>
          </form>

          {workspaces.length > 1 && active !== null && (
            <div className="flex justify-end">
              <button
                type="button"
                className={BTN_DANGER}
                onClick={() => {
                  onRemove(active.id);
                  close();
                }}
              >
                Remove “{active.name}”
              </button>
            </div>
          )}
        </div>
      )}
    </Popover>
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
