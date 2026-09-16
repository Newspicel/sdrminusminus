import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { TuneTarget } from "../canvas/libraryTarget";
import { BOOKMARKS_KEY, bookmarksQuery, createBookmark, deleteBookmark } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { Bookmark, CreateBookmarkRequest } from "../lib/types";
import { sameMode, useTuner } from "../lib/useTuner";
import { Button, Form, Input } from "./BaseControls";
import { BTN, FIELD } from "./controls";
import { formatMhz } from "./format";

export function BookmarksPanel({ target }: { target: TuneTarget | null }) {
  const queryClient = useQueryClient();
  const bookmarks = useQuery(bookmarksQuery());
  const { tune, frequencyHz, channelType, ready } = useTuner(target);
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

  const sorted = (bookmarks.data ?? []).toSorted((a, b) => a.freq_hz - b.freq_hz);

  const recall = (bookmark: Bookmark): void => {
    tune(bookmark.freq_hz);
    const saved = bookmark.mode;
    if (saved != null && channelType !== null && !sameMode(saved, channelType)) {
      pushToast(
        `${saved.toUpperCase()} is the mode for this bookmark, not ${channelType.toUpperCase()}`,
        "info",
      );
    }
  };

  return (
    <div className="flex flex-col gap-2 p-3">
      {target === null && (
        <span className="text-sm text-ink-dim">Select a Device or decoder first.</span>
      )}
      <Form
        className="flex flex-wrap gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (frequencyHz != null && label.trim() !== "") {
            addMut.mutate({
              freq_hz: frequencyHz,
              label: label.trim(),
              mode: mode.trim() === "" ? null : mode.trim(),
            });
          }
        }}
      >
        <Input
          className={`${FIELD} min-w-0 flex-1`}
          placeholder="Label current frequency"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          aria-label="Bookmark label"
        />
        <Input
          className={`${FIELD} w-16`}
          placeholder="mode"
          value={mode}
          onChange={(e) => setMode(e.target.value)}
          aria-label="Bookmark mode"
        />
        <Button
          type="submit"
          className={BTN}
          disabled={frequencyHz == null || label.trim() === "" || addMut.isPending}
        >
          Save
        </Button>
      </Form>

      {target !== null && !ready && sorted.length > 0 && (
        <span className="text-sm text-ink-dim">Tuning is locked here.</span>
      )}

      {sorted.map((b) => (
        <div key={b.id} className="flex items-center gap-2">
          <Button
            type="button"
            className="min-w-0 flex-1 rounded px-1 py-1 text-left transition-colors hover:bg-panel-2 disabled:opacity-40 max-md:min-h-10"
            disabled={!ready}
            onClick={() => recall(b)}
          >
            <span className="font-mono text-sm tabular-nums text-ink">{formatMhz(b.freq_hz)}</span>
            <span className="ml-2 text-sm text-ink-dim">{b.label}</span>
            {b.mode != null && b.mode !== "" && (
              <span className="ml-2 rounded border border-line px-1 font-mono text-[10px] uppercase text-ink-dim">
                {b.mode}
              </span>
            )}
          </Button>
          <Button
            type="button"
            className={`${BTN} hover:border-danger hover:text-danger`}
            disabled={deleteMut.isPending}
            onClick={() => deleteMut.mutate(b.id)}
          >
            Delete
          </Button>
        </div>
      ))}
      {bookmarks.data?.length === 0 && (
        <span className="text-sm text-ink-dim">No bookmarks yet.</span>
      )}
    </div>
  );
}
