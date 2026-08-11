// Preset save/apply/delete (PLAN §11). The list is WS-invalidated (scope "presets");
// rejections surface inline like the device PATCH banner.
//
// A preset is the whole workspace, not one radio (`PresetSnapshot`): saving takes every radio the
// patch has open, and applying puts each of them back — matched to the node that drew it. So
// nothing here names a target, and there is nothing to select before pressing Save.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  applyPreset,
  createPreset,
  deletePreset,
  PRESETS_KEY,
  presetsQuery,
  STATE_KEY,
} from "../lib/api";
import { pushToast } from "../lib/toasts";
import { BTN, FIELD } from "./controls";

export function PresetsPanel() {
  const queryClient = useQueryClient();
  const presets = useQuery(presetsQuery());
  const [name, setName] = useState("");

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: PRESETS_KEY });
  };
  const saveMut = useMutation({
    mutationFn: (preset: string) => createPreset(preset),
    onSuccess: () => {
      setName("");
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });
  const applyMut = useMutation({
    mutationFn: applyPreset,
    onError: (e) => pushToast(e.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });
  const deleteMut = useMutation({
    mutationFn: deletePreset,
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  return (
    <div className="flex flex-col gap-2 p-3">
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim() !== "") {
            saveMut.mutate(name.trim());
          }
        }}
      >
        <input
          className={`${FIELD} min-w-0 flex-1`}
          placeholder="Name this bench"
          value={name}
          onChange={(e) => setName(e.target.value)}
          aria-label="Preset name"
        />
        <button type="submit" className={BTN} disabled={name.trim() === "" || saveMut.isPending}>
          Save
        </button>
      </form>

      {(presets.data ?? []).map((p) => (
        <div key={p.id} className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm text-ink">{p.name}</div>
            <div className="truncate font-mono text-[10px] tabular-nums text-ink-dim">
              {p.devices} radio{p.devices === 1 ? "" : "s"}
            </div>
          </div>
          <button
            type="button"
            className={BTN}
            disabled={applyMut.isPending}
            onClick={() => applyMut.mutate(p.id)}
          >
            Apply
          </button>
          <button
            type="button"
            className={`${BTN} hover:border-danger hover:text-danger`}
            disabled={deleteMut.isPending}
            onClick={() => deleteMut.mutate(p.id)}
          >
            Delete
          </button>
        </div>
      ))}
      {presets.data?.length === 0 && (
        <span className="text-sm text-ink-dim">
          No presets saved. Saving one takes every radio this workspace has open, where it is now.
        </span>
      )}
    </div>
  );
}
