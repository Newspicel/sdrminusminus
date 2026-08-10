// The one row of chrome above the station: which workspace, which view, what to add, and the
// library of things that are not nodes (presets, bookmarks, templates, recordings).
//
// Everything that *is* a node lives on the canvas — this bar deliberately holds no radio
// controls, because the receiver node is where a radio is operated now (CANVAS §1).
import { useState } from "react";
import { BookmarksPanel } from "../components/BookmarksPanel";
import { BTN_QUIET, ICON_BTN, LABEL, segment } from "../components/controls";
import { Popover } from "../components/Popover";
import { PresetsPanel } from "../components/PresetsPanel";
import { RecordingsPanel } from "../components/RecordingsPanel";
import { TemplatesPanel } from "../components/TemplatesPanel";
import { ThemeControl } from "../components/ThemeControl";
import type { NodeKind, PatchNode, WorkspaceInfo } from "../lib/types";
import { useStationContext } from "./context";
import { newNodeId } from "./graph";

export type View = "patch" | "rack";

/** Where a node dropped from the palette lands: to the right of everything already drawn, so it
 * is on screen and on top of nothing. CANVAS §9 left auto-placement to feel; this is the feel. */
const DROP_STEP = 40;

export function StationBar({
  view,
  onView,
  workspaces,
  activeWorkspace,
  onActivate,
  onCreate,
  onRemove,
  connected,
  clients,
  onShowShortcuts,
}: {
  view: View;
  onView: (view: View) => void;
  workspaces: readonly WorkspaceInfo[];
  activeWorkspace: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRemove: (id: number) => void;
  connected: boolean;
  clients: number;
  onShowShortcuts: () => void;
}) {
  const station = useStationContext();
  const active = workspaces.find((workspace) => workspace.id === activeWorkspace) ?? null;

  const add = (kind: NodeKind, channelType?: string) => {
    const id = newNodeId(kind);
    station.edit((snapshot) => {
      const drawn = snapshot.graph.nodes;
      const x = drawn.reduce((max, node) => Math.max(max, node.position.x), 0) + 360;
      const y = drawn.length * DROP_STEP;
      const node = {
        id,
        position: { x, y },
        ...(kind === "channel"
          ? { kind: "channel" as const, data: { channel_type: channelType ?? "nfm" } }
          : kind === "device"
            ? { kind: "device" as const, data: {} }
            : { kind }),
      } as PatchNode;
      return { ...snapshot, graph: { ...snapshot.graph, nodes: [...drawn, node] } };
    });
    station.select(id);
  };

  return (
    <header className="flex h-9 shrink-0 items-center gap-2 border-b border-line bg-panel px-2">
      <span className="font-mono text-sm tracking-tight text-accent">sdr--</span>

      <Popover
        label={active?.name ?? "No workspace"}
        triggerClass={`${BTN_QUIET} font-mono`}
        width="w-72"
      >
        {(close) => (
          <WorkspaceMenu
            workspaces={workspaces}
            activeWorkspace={activeWorkspace}
            onActivate={(id) => {
              onActivate(id);
              close();
            }}
            onCreate={(name) => {
              onCreate(name);
              close();
            }}
            onRemove={onRemove}
          />
        )}
      </Popover>

      <Popover label="+ Node" triggerClass={BTN_QUIET} width="w-72">
        {(close) => (
          <Palette
            onAdd={(kind, channelType) => {
              add(kind, channelType);
              close();
            }}
          />
        )}
      </Popover>

      <span className="ml-2 inline-flex" role="group" aria-label="View">
        {(["patch", "rack"] as const).map((option) => (
          <button
            key={option}
            type="button"
            className={segment(view === option)}
            aria-pressed={view === option}
            onClick={() => onView(option)}
          >
            {option === "patch" ? "Patch" : "Rack"}
          </button>
        ))}
      </span>

      <span className="ml-auto flex items-center gap-2">
        <Popover label="Library" triggerClass={BTN_QUIET} align="end" width="w-96">
          {() => <Library />}
        </Popover>
        <ThemeControl />
        <span className={LABEL} title={connected ? "Connected" : "Reconnecting"}>
          <span
            aria-hidden
            className={`size-1.5 rounded-full ${connected ? "bg-ok" : "bg-danger"}`}
          />
          {connected ? "link" : "down"}
          <span className="font-mono tabular-nums">{clients}</span>
        </span>
        <button
          type="button"
          className={ICON_BTN}
          aria-label="Keyboard shortcuts"
          onClick={onShowShortcuts}
        >
          ?
        </button>
      </span>
    </header>
  );
}

/** The palette is backend-driven (PLAN §2): the kinds come from `GET /api/patch/catalog` and the
 * channel entries from `GET /api/channeltypes`, so a new node type or decoder appears here with
 * no frontend edit. */
function Palette({ onAdd }: { onAdd: (kind: NodeKind, channelType?: string) => void }) {
  const station = useStationContext();
  return (
    <div className="flex flex-col gap-2">
      {station.context.catalog.nodes.map((entry) =>
        entry.needs_channel_type === true ? (
          <div key={entry.kind} className="flex flex-col gap-1">
            <span className={LABEL}>{entry.name}</span>
            <div className="flex flex-wrap gap-1">
              {station.context.channelTypes.map((type) => (
                <button
                  key={type.type_id}
                  type="button"
                  className={BTN_QUIET}
                  onClick={() => onAdd("channel", type.type_id)}
                >
                  {type.name}
                </button>
              ))}
            </div>
          </div>
        ) : (
          <button
            key={entry.kind}
            type="button"
            className={`${BTN_QUIET} justify-start`}
            onClick={() => onAdd(entry.kind as NodeKind)}
          >
            {entry.name}
          </button>
        ),
      )}
    </div>
  );
}

/** Presets, bookmarks, templates and recordings are station *config*, not nodes on the patch —
 * they configure the radios the nodes name. They live in one drawer rather than as node kinds
 * with no stream to carry. */
function Library() {
  const station = useStationContext();
  const [tab, setTab] = useState<"presets" | "bookmarks" | "templates" | "recordings">("templates");
  // These panels act on one radio, and applying a template or a preset to the wrong one is not
  // recoverable by undo. The target is the selected receiver node; with nothing selected it
  // falls back only when there is exactly one radio to mean, and otherwise the drawer says so
  // instead of silently picking the first.
  //
  // "One radio to mean" counts the radios *this patch names*, not every set the engine has open:
  // applying a patch never closes anything (CANVAS §4), so a workspace with nothing drawn on it
  // still sits beside whatever the last one left running — and the drawer offering to retune a
  // radio the operator can no longer see on the canvas is how a preset lands on the wrong one.
  const selected =
    station.selected === null ? null : (station.devices.get(station.selected) ?? null);
  const drawn = [...station.devices.values()];
  const only = drawn.length === 1 ? (drawn[0] ?? null) : null;
  const active = selected ?? only;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex" role="group" aria-label="Library section">
        {(["templates", "presets", "bookmarks", "recordings"] as const).map((option) => (
          <button
            key={option}
            type="button"
            className={segment(tab === option)}
            aria-pressed={tab === option}
            onClick={() => setTab(option)}
          >
            {option[0]?.toUpperCase()}
            {option.slice(1)}
          </button>
        ))}
      </div>
      <span className={LABEL}>
        {active === null
          ? drawn.length > 1
            ? "select a receiver node to choose the target"
            : "no receiver on this patch"
          : `on ${active.device.label}`}
      </span>
      {tab === "templates" && <TemplatesPanel active={active} onApplied={() => station.apply()} />}
      {tab === "presets" && <PresetsPanel active={active} />}
      {tab === "bookmarks" && <BookmarksPanel active={active} />}
      {tab === "recordings" && <RecordingsPanel onSelect={() => station.apply()} />}
    </div>
  );
}

function WorkspaceMenu({
  workspaces,
  activeWorkspace,
  onActivate,
  onCreate,
  onRemove,
}: {
  workspaces: readonly WorkspaceInfo[];
  activeWorkspace: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRemove: (id: number) => void;
}) {
  const [name, setName] = useState("");
  return (
    <div className="flex flex-col gap-2">
      <span className={LABEL}>Workspaces</span>
      {workspaces.map((workspace) => (
        <div key={workspace.id} className="flex items-center gap-1">
          <button
            type="button"
            className={`${segment(workspace.id === activeWorkspace)} flex-1 justify-between`}
            onClick={() => onActivate(workspace.id)}
          >
            <span className="truncate">{workspace.name}</span>
            <span className="font-mono text-[10px] text-ink-faint tabular-nums">
              {workspace.nodes}
            </span>
          </button>
          <button
            type="button"
            className={ICON_BTN}
            aria-label={`Delete ${workspace.name}`}
            onClick={() => onRemove(workspace.id)}
          >
            ✕
          </button>
        </div>
      ))}
      <form
        className="flex gap-1"
        onSubmit={(event) => {
          event.preventDefault();
          if (name.trim() !== "") {
            onCreate(name.trim());
            setName("");
          }
        }}
      >
        <input
          className="h-7 min-w-0 flex-1 rounded-[3px] border border-line-strong bg-panel-2 px-2 font-mono text-xs text-ink placeholder:text-ink-faint"
          placeholder="New workspace"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <button type="submit" className={BTN_QUIET}>
          Add
        </button>
      </form>
    </div>
  );
}
