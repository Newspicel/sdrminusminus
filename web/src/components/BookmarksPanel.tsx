import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import { useState } from "react";
import type { TuneTarget } from "../canvas/libraryTarget";
import { BOOKMARKS_KEY, bookmarksQuery, createBookmark, deleteBookmark } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { Bookmark, CreateBookmarkRequest } from "../lib/types";
import { sameMode, useTuner } from "../lib/useTuner";
import { Button, Form, Input } from "./BaseControls";
import { BTN, CHIP_SM, FIELD } from "./controls";
import { formatMhz } from "./format";
import { List, ListRow, Panel, PanelHint, RowAction } from "./ListPanel";

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
    <Panel>
      {target === null && <PanelHint>Select a device or decoder first.</PanelHint>}
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
        <PanelHint>Tuning is locked here.</PanelHint>
      )}
      {bookmarks.data?.length === 0 && <PanelHint>No bookmarks yet.</PanelHint>}
      {sorted.length > 0 && (
        <List>
          {sorted.map((b) => (
            <ListRow
              key={b.id}
              primary={formatMhz(b.freq_hz)}
              badge={
                b.mode != null && b.mode !== "" ? (
                  <span className={CHIP_SM}>{b.mode}</span>
                ) : undefined
              }
              secondary={b.label}
              disabled={!ready}
              onSelect={() => recall(b)}
              actions={
                <RowAction
                  label={`Delete ${b.label}`}
                  glyph={Trash2}
                  danger
                  disabled={deleteMut.isPending}
                  onClick={() => deleteMut.mutate(b.id)}
                />
              }
            />
          ))}
        </List>
      )}
    </Panel>
  );
}
