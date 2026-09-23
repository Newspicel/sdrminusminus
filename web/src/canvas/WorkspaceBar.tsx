import { CircleQuestionMark, Plus, Redo2, Undo2 } from "lucide-react";
import { Button } from "../components/BaseControls";
import {
  BTN_PRIMARY,
  BTN_QUIET,
  ICON_BTN,
  type Options,
  segment,
  WELL,
} from "../components/controls";
import { Icon } from "../components/Icon";
import { Popover } from "../components/Popover";
import { ThemeControl } from "../components/ThemeControl";
import type { WorkspaceInfo } from "../lib/types";
import { useWorkspaceContext } from "./context";
import { Library } from "./Library";
import { NodePalette } from "./NodePalette";
import { useAddNode } from "./useAddNode";
import { WorkspaceMenu } from "./WorkspaceMenu";

export type View = "patch" | "rack";

const VIEWS: Options<View> = [
  { value: "patch", label: "Patch" },
  { value: "rack", label: "Rack" },
];

export function WorkspaceBar({
  view,
  onView,
  workspaces,
  activeWorkspace,
  onActivate,
  onCreate,
  onRename,
  onClone,
  onImport,
  onRemove,
  onUndo,
  onRedo,
  canUndo,
  canRedo,
  onShowShortcuts,
  onOpenTool,
}: {
  view: View;
  onView: (view: View) => void;
  workspaces: readonly WorkspaceInfo[];
  activeWorkspace: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRename: (id: number, name: string) => void;
  onClone: (id: number) => void;
  onImport: (file: File) => void;
  onRemove: (id: number) => void;
  onUndo: () => void;
  onRedo: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onShowShortcuts: () => void;
  onOpenTool: (id: string) => void;
}) {
  const workspace = useWorkspaceContext();
  const add = useAddNode();
  const active = workspaces.find((entry) => entry.id === activeWorkspace) ?? null;
  const pinned = workspace.rack.slots?.length ?? 0;

  return (
    <header className="flex h-9 shrink-0 items-center gap-1 border-b border-line bg-panel px-2">
      <img src="/icon.svg" alt="" width={20} height={20} className="shrink-0" />
      <span className="mr-1 font-mono text-sm font-medium tracking-tight text-accent">SDR--</span>

      <Popover
        label={active?.name ?? "No workspace"}
        triggerClass={`${BTN_QUIET} font-mono`}
        width="w-80"
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
            onRename={onRename}
            onClone={onClone}
            onImport={(file) => {
              onImport(file);
              close();
            }}
            onRemove={(id) => {
              onRemove(id);
              if (id === activeWorkspace) {
                close();
              }
            }}
          />
        )}
      </Popover>

      <Rule />

      <span className={WELL} role="group" aria-label="View">
        {VIEWS.map((option) => (
          <Button
            key={option.value}
            type="button"
            className={segment(view === option.value)}
            aria-pressed={view === option.value}
            onClick={() => onView(option.value)}
          >
            {option.label}
            {option.value === "rack" && pinned > 0 && (
              <span aria-hidden className="text-[10px] text-ink-faint tabular-nums">
                {pinned}
              </span>
            )}
          </Button>
        ))}
      </span>

      <Rule />

      <Popover
        label={
          <>
            <Icon glyph={Plus} />
            Add
          </>
        }
        title="Add a node, or double-click the canvas"
        triggerClass={`${BTN_PRIMARY} pl-2`}
        width="w-[44rem]"
        padded={false}
      >
        {(close) => (
          <NodePalette
            onAdd={(kind, channelType) => {
              add(kind, channelType);
              close();
            }}
          />
        )}
      </Popover>

      <span className="ml-auto flex items-center gap-1">
        <span className="flex items-center" role="group" aria-label="History">
          <Button
            type="button"
            className={ICON_BTN}
            aria-label="Undo the last change to the workspace"
            disabled={!canUndo}
            onClick={onUndo}
          >
            <Icon glyph={Undo2} />
          </Button>
          <Button
            type="button"
            className={ICON_BTN}
            aria-label="Redo the last undone change"
            disabled={!canRedo}
            onClick={onRedo}
          >
            <Icon glyph={Redo2} />
          </Button>
        </span>
        <Rule />
        <Popover
          label="Library"
          triggerClass={BTN_QUIET}
          align="end"
          width="w-[44rem]"
          padded={false}
        >
          {(close) => (
            <Library
              onOpenTool={(id) => {
                close();
                onOpenTool(id);
              }}
            />
          )}
        </Popover>
        <Rule />
        <ThemeControl />
        <Button
          type="button"
          className={ICON_BTN}
          aria-label="Keyboard shortcuts, licenses and problem reports"
          onClick={onShowShortcuts}
        >
          <Icon glyph={CircleQuestionMark} />
        </Button>
      </span>
    </header>
  );
}

function Rule() {
  return <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-line" />;
}
