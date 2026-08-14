import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { Input } from "@/components/ui/input";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemGroup,
  ItemTitle,
} from "@/components/ui/item";
import {
  applyPreset,
  createPreset,
  deletePreset,
  PRESETS_KEY,
  presetsQuery,
  STATE_KEY,
} from "../lib/api";
import { pushToast } from "../lib/toasts";
import { EmptyState } from "./EmptyState";

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
        className="flex"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim() !== "") {
            saveMut.mutate(name.trim());
          }
        }}
      >
        <ButtonGroup className="w-full">
          <Input
            placeholder="Name this bench"
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-label="Preset name"
          />
          <Button type="submit" disabled={name.trim() === "" || saveMut.isPending}>
            Save
          </Button>
        </ButtonGroup>
      </form>

      <ItemGroup>
        {(presets.data ?? []).map((p) => (
          <Item key={p.id} size="xs">
            <ItemContent>
              <ItemTitle>{p.name}</ItemTitle>
              <ItemDescription className="font-mono text-[10px] tabular-nums">
                {p.devices} radio{p.devices === 1 ? "" : "s"}
              </ItemDescription>
            </ItemContent>
            <ItemActions>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={applyMut.isPending}
                onClick={() => applyMut.mutate(p.id)}
              >
                Apply
              </Button>
              <Button
                type="button"
                variant="destructive"
                size="sm"
                disabled={deleteMut.isPending}
                onClick={() => deleteMut.mutate(p.id)}
              >
                Delete
              </Button>
            </ItemActions>
          </Item>
        ))}
      </ItemGroup>
      {presets.data?.length === 0 && (
        <EmptyState>
          No presets saved. Saving one takes every radio this workspace has open, where it is now.
        </EmptyState>
      )}
    </div>
  );
}
