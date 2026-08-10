// Preset save/apply/delete (PLAN §11). The list is WS-invalidated (scope "presets");
// rejections surface inline like the device PATCH banner.
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
import type { DeviceSet } from "../lib/types";
import { BTN, FIELD } from "./controls";

export function PresetsPanel({ active }: { active: DeviceSet | null }) {
  const queryClient = useQueryClient();
  const presets = useQuery(presetsQuery());
  const [name, setName] = useState("");

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: PRESETS_KEY });
  };
  const saveMut = useMutation({
    mutationFn: (v: { name: string; ds: number }) => createPreset(v.name, v.ds),
    onSuccess: () => {
      setName("");
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });
  const applyMut = useMutation({
    mutationFn: (v: { id: number; ds: number }) => applyPreset(v.id, v.ds),
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
          if (active && name.trim() !== "") {
            saveMut.mutate({ name: name.trim(), ds: active.id });
          }
        }}
      >
        <input
          className={`${FIELD} min-w-0 flex-1`}
          placeholder="Preset name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          aria-label="Preset name"
        />
        <button
          type="submit"
          className={BTN}
          disabled={!active || name.trim() === "" || saveMut.isPending}
        >
          Save
        </button>
      </form>

      {(presets.data ?? []).map((p) => (
        <div key={p.id} className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm text-ink">{p.name}</div>
            <div className="truncate font-mono text-[10px] text-ink-dim">{p.device_id}</div>
          </div>
          <button
            type="button"
            className={BTN}
            disabled={!active || applyMut.isPending}
            onClick={() => active && applyMut.mutate({ id: p.id, ds: active.id })}
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
        <span className="text-sm text-ink-dim">No presets saved.</span>
      )}
    </div>
  );
}
