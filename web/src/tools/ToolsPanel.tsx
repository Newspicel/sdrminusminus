import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { Button } from "../components/BaseControls";
import { LABEL } from "../components/controls";
import { toolsQuery } from "../lib/api";
import { groupTools, launchableTools } from "./registry";

const ROW =
  "w-full rounded-[3px] px-1 py-1 text-left transition-colors duration-100 hover:bg-panel-2";

export function ToolsPanel({ onOpen }: { onOpen: (id: string) => void }) {
  const tools = useQuery(toolsQuery());
  const groups = useMemo(
    () => groupTools(launchableTools(tools.data?.tools ?? [])),
    [tools.data?.tools],
  );

  if (tools.isError) {
    return <p className="p-3 text-sm text-danger">Could not load the tools.</p>;
  }
  if (groups.length === 0) {
    return (
      <p className="p-3 text-sm text-ink-dim">
        {tools.isPending ? "Loading the tools…" : "This build has no tools."}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-3 p-3">
      {groups.map((group) => (
        <section key={group.category}>
          <h3 className={`${LABEL} px-1 pb-1`}>{group.label}</h3>
          <ul className="flex flex-col">
            {group.tools.map(({ descriptor }) => (
              <li key={descriptor.id}>
                <Button type="button" className={ROW} onClick={() => onOpen(descriptor.id)}>
                  <span className="min-w-0 truncate text-sm text-ink">{descriptor.name}</span>
                </Button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
