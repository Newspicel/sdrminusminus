import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { BOOKMARKS_KEY, bookmarksQuery, createBookmark, deleteBookmark } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { CreateBookmarkRequest, DeviceSet } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { BTN, FIELD } from "./controls";
import { formatMhz } from "./format";

export function BookmarksPanel({ active }: { active: DeviceSet | null }) {
  const queryClient = useQueryClient();
  const bookmarks = useQuery(bookmarksQuery());
  const { applyPatch } = useDevicePatch();
  const [label, setLabel] = useState("");
  const [mode, setMode] = useState("");

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: BOOKMARKS_KEY });
  };
  const addMut = useMutation({
    mutationFn: (v: CreateBookmarkRequest) => createBookmark(v),
    onSuccess: () => {
      setLabel("");
      setMode("");
    },
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });
  const deleteMut = useMutation({
    mutationFn: deleteBookmark,
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const centerHz = active?.settings.center_hz;
  // `toSorted` wants lib es2023 (tsconfig pins es2022); the spread already prevents the
  // mutation the rule guards against.
  // oxlint-disable-next-line unicorn/no-array-sort
  const sorted = [...(bookmarks.data ?? [])].sort((a, b) => a.freq_hz - b.freq_hz);

  return (
    <div className="flex flex-col gap-2 p-3">
      {active === null && (
        <span className="text-sm text-ink-dim">
          Nothing to tune or save from: select a device node on the canvas first.
        </span>
      )}
      <form
        className="flex flex-wrap gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (centerHz != null && label.trim() !== "") {
            addMut.mutate({
              freq_hz: centerHz,
              label: label.trim(),
              mode: mode.trim() === "" ? null : mode.trim(),
            });
          }
        }}
      >
        <input
          className={`${FIELD} min-w-0 flex-1`}
          placeholder="Label current frequency"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          aria-label="Bookmark label"
        />
        <input
          className={`${FIELD} w-16`}
          placeholder="mode"
          value={mode}
          onChange={(e) => setMode(e.target.value)}
          aria-label="Bookmark mode"
        />
        <button
          type="submit"
          className={BTN}
          disabled={centerHz == null || label.trim() === "" || addMut.isPending}
        >
          Save
        </button>
      </form>

      {sorted.map((b) => (
        <div key={b.id} className="flex items-center gap-2">
          <button
            type="button"
            className="min-w-0 flex-1 rounded px-1 py-1 text-left transition-colors hover:bg-panel-2 disabled:opacity-40 max-md:min-h-10"
            disabled={!active}
            onClick={() => active && applyPatch(active.id, { center_hz: b.freq_hz })}
          >
            <span className="font-mono text-sm tabular-nums text-ink">{formatMhz(b.freq_hz)}</span>
            <span className="ml-2 text-sm text-ink-dim">{b.label}</span>
            {b.mode != null && b.mode !== "" && (
              <span className="ml-2 rounded border border-line px-1 font-mono text-[10px] uppercase text-ink-dim">
                {b.mode}
              </span>
            )}
          </button>
          <button
            type="button"
            className={`${BTN} hover:border-danger hover:text-danger`}
            disabled={deleteMut.isPending}
            onClick={() => deleteMut.mutate(b.id)}
          >
            Delete
          </button>
        </div>
      ))}
      {bookmarks.data?.length === 0 && (
        <span className="text-sm text-ink-dim">No bookmarks yet.</span>
      )}
    </div>
  );
}
