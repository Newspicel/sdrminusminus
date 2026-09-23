import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { List, ListRow, Panel, PanelHint } from "../components/ListPanel";
import { toolsQuery } from "../lib/api";
import { groupTools, launchableTools } from "./registry";

export function ToolsPanel({ onOpen }: { onOpen: (id: string) => void }) {
  const tools = useQuery(toolsQuery());
  const groups = useMemo(
    () => groupTools(launchableTools(tools.data?.tools ?? [])),
    [tools.data?.tools],
  );

  if (tools.isError) {
    return (
      <Panel>
        <PanelHint>Could not load the tools.</PanelHint>
      </Panel>
    );
  }
  if (groups.length === 0) {
    return (
      <Panel>
        <PanelHint>{tools.isPending ? "Loading the tools…" : "This build has no tools."}</PanelHint>
      </Panel>
    );
  }

  return (
    <Panel>
      {groups.map((group) => (
        <List key={group.category} title={group.label}>
          {group.tools.map(({ descriptor }) => (
            <ListRow
              key={descriptor.id}
              primary={descriptor.name}
              onSelect={() => onOpen(descriptor.id)}
            />
          ))}
        </List>
      ))}
    </Panel>
  );
}
