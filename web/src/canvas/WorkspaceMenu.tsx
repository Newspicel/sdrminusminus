import { type ReactNode, useState } from "react";
import { Button, Form, Input } from "../components/BaseControls";
import { Checkbox } from "../components/Checkbox";
import { BTN_QUIET, FIELD, ICON_BTN, LABEL, segment } from "../components/controls";
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
              className={`${segment(workspace.id === activeWorkspace)} flex-1 justify-between`}
              onClick={() => onActivate(workspace.id)}
            >
              <span className="truncate">{workspace.name}</span>
              <span className="font-mono text-[10px] text-ink-faint tabular-nums">
                {workspace.nodes}
              </span>
            </Button>
            <Button
              type="button"
              className={ICON_BTN}
              aria-label={`Delete ${workspace.name}`}
              onClick={() => onRemove(workspace.id)}
            >
              ✕
            </Button>
          </div>
        ))}
        <Form
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
            className={`${FIELD} flex-1`}
            placeholder="New workspace"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
          <Button type="submit" className={BTN_QUIET}>
            Add
          </Button>
        </Form>
      </div>

      {/* Settings of the workspace, not of the app or of the browser looking at it: they travel
          with the snapshot, so every client on this server sees the same answer. The band plan is
          the first section; later ones stack under the same heading. */}
      <div className="flex flex-col gap-2 border-t border-line pt-3">
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
    <div className="flex flex-col gap-1.5 rounded-[3px] border border-line bg-panel-2 p-2">
      <span className="text-xs font-medium text-ink">{title}</span>
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
      <label className="flex cursor-pointer items-center gap-2 text-xs text-ink-dim">
        <Checkbox checked={ruler} onChange={setRuler} />
        Draw the ruler on every scope
      </label>
    </div>
  );
}
