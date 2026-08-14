import { type ReactNode, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "../components/Checkbox";
import { LABEL } from "../components/controls";
import { Select } from "../components/Select";
import type { WorkspaceInfo } from "../lib/types";
import { useBandPlan } from "../lib/useBandPlan";

export function WorkspaceMenu({
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
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <span className={LABEL}>Workspaces</span>
        {workspaces.map((workspace) => (
          <div key={workspace.id} className="flex items-center gap-1">
            <Button
              type="button"
              variant={workspace.id === activeWorkspace ? "secondary" : "ghost"}
              size="sm"
              className="flex-1 justify-between"
              onClick={() => onActivate(workspace.id)}
            >
              <span className="truncate">{workspace.name}</span>
              <span className="font-mono text-[10px] text-muted-foreground/70 tabular-nums">
                {workspace.nodes}
              </span>
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={`Delete ${workspace.name}`}
              onClick={() => onRemove(workspace.id)}
            >
              ✕
            </Button>
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
          <Input
            className="flex-1"
            placeholder="New workspace"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
          <Button type="submit" variant="ghost" size="sm">
            Add
          </Button>
        </form>
      </div>

      {/* Settings of the workspace, not of the app or of the browser looking at it: they travel
          with the snapshot, so every client on this server sees the same answer. The band plan is
          the first section; later ones stack under the same heading. */}
      <div className="flex flex-col gap-2 border-t border-border pt-3">
        <span className={LABEL}>Workspace settings</span>
        <Setting title="Band plan">
          <BandSettings />
        </Setting>
      </div>
    </div>
  );
}

/** One named block under the settings heading. */
function Setting({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 rounded-[3px] border border-border bg-muted p-2">
      <span className="text-xs font-medium text-foreground">{title}</span>
      {children}
    </div>
  );
}

function BandSettings() {
  const { region, regions, ruler, setRegion, setRuler } = useBandPlan();

  return (
    <div className="flex flex-col gap-2">
      <Select
        label="Band plan region"
        value={region ?? ""}
        options={regions.map((entry) => ({ value: entry.id, label: entry.name }))}
        onChange={setRegion}
      />
      <Label className="flex cursor-pointer items-center gap-2 text-xs text-muted-foreground">
        <Checkbox checked={ruler} onChange={setRuler} />
        Draw the ruler on every scope
      </Label>
    </div>
  );
}
