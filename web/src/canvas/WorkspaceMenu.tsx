import { type ReactNode, useState } from "react";
import { Button, Form, Input } from "../components/BaseControls";
import { Checkbox } from "../components/Checkbox";
import { BTN_QUIET, FIELD, ICON_BTN, LABEL, segment } from "../components/controls";
import { Select } from "../components/Select";
import { SettingRow, Settings } from "../components/Settings";
import { workspaceExportUrl } from "../lib/api";
import { pickFile } from "../lib/pickFile";
import type { WorkspaceInfo } from "../lib/types";
import { useBandPlan } from "../lib/useBandPlan";
import { WORKSPACE_FILE_ACCEPT } from "./workspaceExport";

export function WorkspaceMenu({
  workspaces,
  activeWorkspace,
  onActivate,
  onCreate,
  onImport,
  onRemove,
}: {
  workspaces: readonly WorkspaceInfo[];
  activeWorkspace: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onImport: (file: File) => void;
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
            <a
              className={ICON_BTN}
              href={workspaceExportUrl(workspace.id)}
              download
              aria-label={`Export ${workspace.name}`}
              title="Download this workspace as a file"
            >
              ↓
            </a>
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
        <Button
          type="button"
          className={`${BTN_QUIET} justify-center`}
          title="Read a workspace file exported here or on another machine"
          onClick={() => {
            void pickFile(WORKSPACE_FILE_ACCEPT).then((file) => {
              if (file !== null) {
                onImport(file);
              }
            });
          }}
        >
          Import a workspace file
        </Button>
      </div>

      <div className="flex flex-col gap-2 border-t border-line pt-3">
        <span className={LABEL}>Workspace settings</span>
        <Setting title="Band plan">
          <BandSettings />
        </Setting>
      </div>
    </div>
  );
}

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
    <Settings>
      <SettingRow label="Region">
        <Select
          label="Band plan region"
          value={region ?? ""}
          options={regions.map((entry) => ({ value: entry.id, label: entry.name }))}
          onChange={setRegion}
        />
      </SettingRow>
      <SettingRow label="Ruler">
        <Checkbox label="Draw the ruler on every scope" checked={ruler} onChange={setRuler} />
        <span className="text-xs text-ink-dim">Draw it on every scope</span>
      </SettingRow>
    </Settings>
  );
}
