import { Copy, Download, Pencil, X } from "lucide-react";
import { type ReactNode, useRef, useState } from "react";
import { Button, Form, Input } from "../components/BaseControls";
import { Checkbox } from "../components/Checkbox";
import {
  BTN_DANGER_SM,
  BTN_QUIET,
  BTN_SM,
  FIELD,
  ICON_BTN_SM,
  LABEL,
  segment,
} from "../components/controls";
import { Icon } from "../components/Icon";
import { Select } from "../components/Select";
import { SettingRow, Settings } from "../components/Settings";
import { workspaceExportUrl } from "../lib/api";
import { pickFile } from "../lib/pickFile";
import type { WorkspaceInfo } from "../lib/types";
import { useBandPlan } from "../lib/useBandPlan";
import { MAX_NAME_LEN } from "./graph";
import { WORKSPACE_FILE_ACCEPT } from "./workspaceExport";

export function WorkspaceMenu({
  workspaces,
  activeWorkspace,
  onActivate,
  onCreate,
  onRename,
  onClone,
  onImport,
  onRemove,
}: {
  workspaces: readonly WorkspaceInfo[];
  activeWorkspace: number | null;
  onActivate: (id: number) => void;
  onCreate: (name: string) => void;
  onRename: (id: number, name: string) => void;
  onClone: (id: number) => void;
  onImport: (file: File) => void;
  onRemove: (id: number) => void;
}) {
  const [name, setName] = useState("");
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <span className={LABEL}>Workspaces</span>
        {workspaces.map((workspace) => (
          <WorkspaceRow
            key={workspace.id}
            workspace={workspace}
            active={workspace.id === activeWorkspace}
            onActivate={onActivate}
            onRename={onRename}
            onClone={onClone}
            onRemove={onRemove}
          />
        ))}
        <div className="mt-1 flex flex-col gap-1 border-t border-line pt-2">
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
              aria-label="Name for a new workspace"
              placeholder="New workspace"
              maxLength={MAX_NAME_LEN}
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
            <Button type="submit" className={BTN_QUIET} disabled={name.trim() === ""}>
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

function WorkspaceRow({
  workspace,
  active,
  onActivate,
  onRename,
  onClone,
  onRemove,
}: {
  workspace: WorkspaceInfo;
  active: boolean;
  onActivate: (id: number) => void;
  onRename: (id: number, name: string) => void;
  onClone: (id: number) => void;
  onRemove: (id: number) => void;
}) {
  const [mode, setMode] = useState<"idle" | "rename" | "confirm">("idle");
  const [draft, setDraft] = useState(workspace.name);
  const abandoned = useRef(false);

  if (mode === "rename") {
    const finish = () => {
      setMode("idle");
      const next = draft.trim();
      if (!abandoned.current && next !== "" && next !== workspace.name) {
        onRename(workspace.id, next);
      }
    };
    return (
      <Input
        autoFocus
        className={`${FIELD} w-full`}
        aria-label={`New name for ${workspace.name}`}
        maxLength={MAX_NAME_LEN}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={(event) => event.currentTarget.select()}
        onBlur={finish}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.currentTarget.blur();
          }
          if (event.key === "Escape") {
            event.stopPropagation();
            abandoned.current = true;
            event.currentTarget.blur();
          }
        }}
      />
    );
  }

  if (mode === "confirm") {
    return (
      <div className="flex h-7 items-center gap-1 rounded-[3px] border border-danger bg-danger/10 px-2">
        <span className="min-w-0 flex-1 truncate text-xs text-danger">
          Delete {workspace.name}?
        </span>
        <Button
          type="button"
          className={BTN_DANGER_SM}
          title="The workspace, its layout and its history are removed"
          onClick={() => {
            setMode("idle");
            onRemove(workspace.id);
          }}
        >
          Delete
        </Button>
        <Button autoFocus type="button" className={BTN_SM} onClick={() => setMode("idle")}>
          Keep
        </Button>
      </div>
    );
  }

  return (
    <div className="group flex items-center gap-1">
      <Button
        type="button"
        className={`${segment(active)} min-w-0 flex-1 justify-start`}
        aria-pressed={active}
        onClick={() => onActivate(workspace.id)}
      >
        <span className="truncate">{workspace.name}</span>
      </Button>
      <span
        aria-hidden
        className="w-4 text-right font-mono text-[10px] text-ink-faint tabular-nums"
        title={`${workspace.nodes} nodes`}
      >
        {workspace.nodes}
      </span>
      <span className="ml-1 flex items-center gap-0.5 opacity-0 transition-opacity duration-100 group-focus-within:opacity-100 group-hover:opacity-100">
        <Button
          type="button"
          className={ICON_BTN_SM}
          aria-label={`Rename ${workspace.name}`}
          title="Rename this workspace"
          onClick={() => {
            abandoned.current = false;
            setDraft(workspace.name);
            setMode("rename");
          }}
        >
          <Icon glyph={Pencil} size={12} />
        </Button>
        <Button
          type="button"
          className={ICON_BTN_SM}
          aria-label={`Duplicate ${workspace.name}`}
          title="Copy this workspace, its patch and the tuning it was left on"
          onClick={() => onClone(workspace.id)}
        >
          <Icon glyph={Copy} size={12} />
        </Button>
        <a
          className={ICON_BTN_SM}
          href={workspaceExportUrl(workspace.id)}
          download
          aria-label={`Export ${workspace.name}`}
          title="Download this workspace as a file"
        >
          <Icon glyph={Download} size={12} />
        </a>
        <Button
          type="button"
          className={ICON_BTN_SM}
          aria-label={`Delete ${workspace.name}`}
          title="Delete this workspace"
          onClick={() => setMode("confirm")}
        >
          <Icon glyph={X} size={12} />
        </Button>
      </span>
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
