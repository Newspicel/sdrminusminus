import { BTN_QUIET, ICON_BTN, type Options, segment } from "../components/controls";
import { Popover } from "../components/Popover";
import { ThemeControl } from "../components/ThemeControl";
import type { NodeKind, PatchNode, PositionSource, WorkspaceInfo } from "../lib/types";
import { useWorkspaceContext } from "./context";
import { addNode, newNodeId } from "./graph";
import { Library } from "./Library";
import { NodePalette } from "./NodePalette";
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
  onShowShortcuts,
  onShowAbout,
}: {
  view: View;
  onView: (view: View) => void;
  workspaces: readonly WorkspaceInfo[];
  activeWorkspace: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRemove: (id: number) => void;
  onShowShortcuts: () => void;
  onShowAbout: () => void;
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
        ...(kind === "channel"
          ? { kind: "channel" as const, data: { channel_type: channelType ?? "nfm" } }
          : kind === "device"
            ? { kind: "device" as const, data: {} }
            : kind === "gps"
              ? { kind: "gps" as const, data: { source: source ?? { type: "device" } } }
              : kind === "dmr_trunk"
                ? {
                    kind: "dmr_trunk" as const,
                    data: { protocol: "auto", retention_seconds: 300 },
                  }
                : { kind }),
      } as PatchNode;
      return { ...snapshot, graph: addNode(snapshot.graph, node) };
    });
    workspace.select(id);
  };

  return (
    <header className="flex h-9 shrink-0 items-center gap-1 border-b border-line bg-panel px-2">
      {/* Decorative: the wordmark beside it is the accessible name, and a second one would
          make every screen reader say the product twice before the first control. */}
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

      {/* Two plain bar buttons like the ones either side of them — the fill marks the view you
          are in, and the rules around the pair are what say the two belong together. A boxed
          segmented control was tried here and read as a foreign object in a row of flat text. */}
      <span className="flex items-center" role="group" aria-label="View">
        {VIEWS.map((option) => (
          <button
            key={option.value}
            type="button"
            className={`${segment(view === option.value)} font-mono`}
            aria-pressed={view === option.value}
            onClick={() => onView(option.value)}
          >
            {option.label}
            {/* How many faces are on the rack, so pinning one from the patch view has an answer
                without switching to it. Hidden from the accessible name — the button is still
                "Rack", and the count is read from the rack itself. */}
            {option.value === "rack" && pinned > 0 && (
              <span aria-hidden className="text-[10px] text-ink-faint tabular-nums">
                {pinned}
              </span>
            )}
          </button>
        ))}
      </span>

      <Rule />

      <Popover label="+ Node" triggerClass={BTN_QUIET} width="w-96">
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
        <Popover
          label="Library"
          triggerClass={BTN_QUIET}
          align="end"
          width="w-[46rem]"
          padded={false}
        >
          {() => <Library />}
        </Popover>
        <Rule />
        <ThemeControl />
        <button
          type="button"
          className={ICON_BTN}
          aria-label="Keyboard shortcuts"
          onClick={onShowShortcuts}
        >
          ?
        </button>
        <button
          type="button"
          className={ICON_BTN}
          aria-label="About sdr-- and its licenses"
          onClick={onShowAbout}
        >
          i
        </button>
      </span>
    </header>
  );
}

/** The separator between the bar's groups. Decorative — the groups are already named by their
 * controls, and a role="separator" would be one more thing read out on the way past. */
function Rule() {
  return <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-line" />;
}
