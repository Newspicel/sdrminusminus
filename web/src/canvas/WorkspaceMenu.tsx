// The workspace menu: which workspace, and the settings that belong to it rather than to a node
// on it.
//
// The band plan is here and not in the library drawer for that reason — which plan is in force is
// a property of the bench, stored in the snapshot (`WorkspaceSettings`), while *searching* it is a
// browse tool and stays in the library beside the bookmarks.
import { type ReactNode, useState } from "react";
import { Checkbox } from "../components/Checkbox";
import { BTN, BTN_QUIET, FIELD, ICON_BTN, LABEL, segment } from "../components/controls";
import { Select } from "../components/Select";
import { locateBandRegion } from "../lib/api";
import { pushToast } from "../lib/toasts";
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
            <button
              type="button"
              className={`${segment(workspace.id === activeWorkspace)} flex-1 justify-between`}
              onClick={() => onActivate(workspace.id)}
            >
              <span className="truncate">{workspace.name}</span>
              <span className="font-mono text-[10px] text-ink-faint tabular-nums">
                {workspace.nodes}
              </span>
            </button>
            <button
              type="button"
              className={ICON_BTN}
              aria-label={`Delete ${workspace.name}`}
              onClick={() => onRemove(workspace.id)}
            >
              ✕
            </button>
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
          <input
            className={`${FIELD} flex-1`}
            placeholder="New workspace"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
          <button type="submit" className={BTN_QUIET}>
            Add
          </button>
        </form>
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

/** Which band plan this workspace reads, and whether its scopes draw the ruler (). */
function BandSettings() {
  const { region, regions, ruler, setRegion, setRuler } = useBandPlan();
  const [locating, setLocating] = useState(false);

  /** Best-effort only: `navigator.geolocation` needs a secure context, and this server is
   * ordinarily plain HTTP on a LAN, so the button is expected to fail in exactly the deployed
   * case. Choosing the region by hand is the primary path; this is a shortcut, and it says so
   * when it cannot work. */
  const detect = (): void => {
    if (!("geolocation" in navigator)) {
      pushToast("This browser offers no location; choose a region instead");
      return;
    }
    setLocating(true);
    navigator.geolocation.getCurrentPosition(
      (position) => {
        void locateBandRegion(position.coords.latitude, position.coords.longitude)
          .then((found) => {
            setRegion(found.region);
            if (found.approximate) {
              pushToast(
                `Only the ITU region could be decided from here — check the region is right`,
              );
            }
          })
          .catch((error: Error) => pushToast(error.message))
          .finally(() => setLocating(false));
      },
      (error) => {
        setLocating(false);
        // The usual cause is not a refusal but an insecure origin, which the browser also
        // reports as "permission denied"; say both rather than blame the operator.
        pushToast(`No location: ${error.message} (needs HTTPS or localhost)`);
      },
      { maximumAge: 600_000, timeout: 10_000 },
    );
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-1">
        <Select
          label="Band plan region"
          value={region ?? ""}
          options={regions.map((entry) => ({ value: entry.id, label: entry.name }))}
          onChange={setRegion}
          className="min-w-0 flex-1"
        />
        <button type="button" className={BTN} onClick={detect} disabled={locating}>
          {locating ? "Locating…" : "Detect"}
        </button>
      </div>
      <label className="flex cursor-pointer items-center gap-2 text-xs text-ink-dim">
        <Checkbox checked={ruler} onChange={setRuler} />
        Draw the ruler on every scope
      </label>
    </div>
  );
}
