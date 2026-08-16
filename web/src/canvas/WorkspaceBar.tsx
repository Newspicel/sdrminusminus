import { Button } from "../components/BaseControls";
import { BTN_QUIET, ICON_BTN, type Options, segment } from "../components/controls";
import { Popover } from "../components/Popover";
import { ThemeControl } from "../components/ThemeControl";
import type { NodeKind, PatchNode, PositionSource, WorkspaceInfo } from "../lib/types";
import { useWorkspaceContext } from "./context";
import { addNode, newNodeId } from "./graph";
import { Library } from "./Library";
import { NodePalette } from "./NodePalette";
import { newNodeBody } from "./newNode";
import { useNodePlacement } from "./placement";
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
  onRemove: (id: number) => void;
  onUndo: () => void;
  onRedo: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onShowShortcuts: () => void;
  onOpenTool: (id: string) => void;
}) {
  const workspace = useWorkspaceContext();
  const placeNode = useNodePlacement();
  const active = workspaces.find((entry) => entry.id === activeWorkspace) ?? null;
  const pinned = workspace.rack.slots?.length ?? 0;

  const add = (kind: NodeKind, channelType?: string, source?: PositionSource) => {
    const id = newNodeId(kind);
    workspace.edit((snapshot) => {
      const node = {
        id,
        position: placeNode(snapshot.graph, kind),
        ...newNodeBody(kind, { channelType, source }),
      } as PatchNode;
      return { ...snapshot, graph: addNode(snapshot.graph, node) };
    });
    workspace.select(id);
  };

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
            onRemove={onRemove}
          />
        )}
      </Popover>

      <Rule />

      <span className="flex items-center" role="group" aria-label="View">
        {VIEWS.map((option) => (
          <Button
            key={option.value}
            type="button"
            className={`${segment(view === option.value)} font-mono`}
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

      <Popover label="+ Node" triggerClass={BTN_QUIET} width="w-[48rem]">
        {(close) => (
          <NodePalette
            onAdd={(kind, channelType, source) => {
              add(kind, channelType, source);
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
            ↶
          </Button>
          <Button
            type="button"
            className={ICON_BTN}
            aria-label="Redo the last undone change"
            disabled={!canRedo}
            onClick={onRedo}
          >
            ↷
          </Button>
        </span>
        <Rule />
        <Popover
          label="Library"
          triggerClass={BTN_QUIET}
          align="end"
          width="w-[46rem]"
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
          aria-label="Keyboard shortcuts and licenses"
          onClick={onShowShortcuts}
        >
          ?
        </Button>
      </span>
    </header>
  );
}

function Rule() {
  return <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-line" />;
}
