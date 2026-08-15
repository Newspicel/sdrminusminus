import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useRef } from "react";
import { BTN, SURFACE } from "../components/controls";
import { PortalContainerProvider } from "../components/PortalContainer";
import { toolsQuery } from "../lib/api";
import { findTool, type LaunchableTool, launchableTools, toolSize } from "./registry";

/** A calculator is read at a glance; an instrument is worked in. The full size leaves only
 * enough margin to keep the dialog reading as a window over the workspace. */
const SIZES = {
  standard: "max-h-[80vh] w-full max-w-3xl",
  full: "h-[94vh] w-[97vw] max-w-none",
} as const;

export function ToolsDialog({ tool, onClose }: { tool: string | null; onClose: () => void }) {
  const tools = useQuery(toolsQuery());
  const active = findTool(launchableTools(tools.data?.tools ?? []), tool);
  const size = SIZES[toolSize(tool)];
  // A tool's own dropdowns portal into the dialog rather than to the body: a popup left at the
  // document root paints under a dialog that sits above it, and the operator clicks the panel
  // behind it instead of the option they aimed at.
  const portalContainer = useRef<HTMLDivElement>(null);

  return (
    <Dialog.Root
      open={tool !== null}
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          ref={portalContainer}
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex ${size} -translate-x-1/2 -translate-y-1/2 flex-col`}
        >
          <PortalContainerProvider container={portalContainer}>
            <div className="flex shrink-0 items-baseline justify-between gap-4 border-b border-line px-4 py-3">
              <Dialog.Title className="text-base font-medium text-ink">
                {active?.descriptor.name ?? "Tool"}
              </Dialog.Title>
              {active === null && (
                <Dialog.Description className="legend">
                  This build no longer offers that tool
                </Dialog.Description>
              )}
            </div>

            <div className="min-h-0 flex-1 overflow-auto p-4">
              {active !== null && <ToolBody tool={active} />}
            </div>

            <div className="flex shrink-0 justify-end border-t border-line px-4 py-3">
              <Dialog.Close className={BTN}>Close</Dialog.Close>
            </div>
          </PortalContainerProvider>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ToolBody({ tool }: { tool: LaunchableTool }) {
  const Panel = tool.panel;
  if (Panel === null) {
    return (
      <p className="text-xs text-ink-dim">
        The server offers <span className="text-ink">{tool.descriptor.name}</span>, but this client
        has no panel for it. It is still reachable over the API.
      </p>
    );
  }
  return <Panel key={tool.descriptor.id} />;
}
