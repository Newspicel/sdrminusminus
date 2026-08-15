import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useMemo, useRef, useState } from "react";
import { Button } from "../components/BaseControls";
import { BTN, LABEL, SURFACE, segment } from "../components/controls";
import { PortalContainerProvider } from "../components/PortalContainer";
import { toolsQuery } from "../lib/api";
import { groupTools, type LaunchableTool, launchableTools, selectTool } from "./registry";

export function ToolsDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const tools = useQuery(toolsQuery());
  const [selected, setSelected] = useState<string | null>(null);
  const groups = useMemo(
    () => groupTools(launchableTools(tools.data?.tools ?? [])),
    [tools.data?.tools],
  );
  const active = selectTool(groups, selected);
  // A tool's own dropdowns portal into the dialog rather than to the body: a popup left at the
  // document root paints under a dialog that sits above it, and the operator clicks the panel
  // behind it instead of the option they aimed at.
  const portalContainer = useRef<HTMLDivElement>(null);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          ref={portalContainer}
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex h-[80vh] w-full max-w-5xl -translate-x-1/2 -translate-y-1/2 flex-col`}
        >
          <PortalContainerProvider container={portalContainer}>
            <div className="flex shrink-0 items-baseline justify-between gap-4 border-b border-line px-4 py-3">
              <Dialog.Title className="text-base font-medium text-ink">
                {active?.descriptor.name ?? "Tools"}
              </Dialog.Title>
              <Dialog.Description className="legend">
                {active?.descriptor.summary ?? "Instruments and calculators beside the receiver"}
              </Dialog.Description>
            </div>

            <div className="flex min-h-0 flex-1">
              <nav className="w-60 shrink-0 overflow-y-auto border-r border-line p-2">
                {tools.isError && (
                  <p className="px-1 text-xs text-danger">Could not load the tools.</p>
                )}
                {groups.length === 0 && !tools.isError && (
                  <p className="px-1 text-xs text-ink-dim">
                    {tools.isPending ? "Loading…" : "This build has no tools."}
                  </p>
                )}
                {groups.map((group) => (
                  <section key={group.category} className="mb-3">
                    <h3 className={`${LABEL} px-1 pb-1`}>{group.label}</h3>
                    <ul className="flex flex-col">
                      {group.tools.map((tool) => (
                        <li key={tool.descriptor.id}>
                          <Button
                            type="button"
                            className={`${segment(tool.descriptor.id === active?.descriptor.id)} w-full justify-start`}
                            aria-pressed={tool.descriptor.id === active?.descriptor.id}
                            onClick={() => setSelected(tool.descriptor.id)}
                          >
                            {tool.descriptor.name}
                          </Button>
                        </li>
                      ))}
                    </ul>
                  </section>
                ))}
              </nav>

              <div className="min-h-0 flex-1 overflow-auto p-4">
                {active === null ? null : <ToolBody tool={active} />}
              </div>
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

/** Remounted per tool, so switching tools does not carry one tool's inputs into the next. */
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
