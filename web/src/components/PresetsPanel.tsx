import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
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
import { Button, Form, Input } from "./BaseControls";
import { BTN, BTN_SM, FIELD } from "./controls";
import { List, ListRow, Panel, PanelHint, RowAction } from "./ListPanel";

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
    <Panel>
      <Form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim() !== "") {
            saveMut.mutate(name.trim());
          }
        }}
      >
        <Input
          className={`${FIELD} min-w-0 flex-1`}
          placeholder="Name this bench"
          value={name}
          onChange={(e) => setName(e.target.value)}
          aria-label="Preset name"
        />
        <Button type="submit" className={BTN} disabled={name.trim() === "" || saveMut.isPending}>
          Save
        </Button>
      </Form>
      {presets.data?.length === 0 && (
        <PanelHint>A preset saves every open radio as it is now.</PanelHint>
      )}
      {(presets.data?.length ?? 0) > 0 && (
        <List>
          {(presets.data ?? []).map((p) => (
            <ListRow
              key={p.id}
              primary={p.name}
              secondary={`${p.devices} radio${p.devices === 1 ? "" : "s"}`}
              actions={
                <>
                  <Button
                    type="button"
                    className={BTN_SM}
                    disabled={applyMut.isPending}
                    onClick={() => applyMut.mutate(p.id)}
                  >
                    Apply
                  </Button>
                  <RowAction
                    label={`Delete ${p.name}`}
                    glyph={Trash2}
                    danger
                    disabled={deleteMut.isPending}
                    onClick={() => deleteMut.mutate(p.id)}
                  />
                </>
              }
            />
          ))}
        </List>
      )}
    </Panel>
  );
}
