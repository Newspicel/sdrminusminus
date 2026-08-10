// The one optimistic PATCH pipeline for channel settings — the counterpart of
// `useDevicePatch`. Shared because three surfaces now edit the same channels: the channel
// panel, a marker dragged across the spectrum, and the keyboard.
//
// `PATCH /channels/{ch}` replaces the whole settings object, so every edit is widened over the
// *optimistic* current value; chained edits therefore accumulate instead of each re-sending a
// stale target.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mergeChannelSettings } from "../components/channelSettings";
import { patchChannel, STATE_KEY } from "./api";
import { pushToast } from "./toasts";
import type { ChannelSettings, StateSnapshot } from "./types";

export type ChannelEdit =
  | Partial<ChannelSettings>
  | ((current: ChannelSettings) => Partial<ChannelSettings>);

export function useChannelPatch(): {
  applyEdit: (ds: number, ch: number, edit: ChannelEdit) => void;
  cachedChannel: (ds: number, ch: number) => ChannelSettings | undefined;
} {
  const queryClient = useQueryClient();
  const patchMut = useMutation({
    mutationFn: (v: { ds: number; ch: number; settings: ChannelSettings }) =>
      patchChannel(v.ds, v.ch, v.settings),
    // A rejected PATCH must be visible, not just snap the control back (CLAUDE.md: no silent
    // failure).
    onError: (error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const cachedChannel = (ds: number, ch: number): ChannelSettings | undefined =>
    queryClient
      .getQueryData<StateSnapshot>(STATE_KEY)
      ?.device_sets.find((set) => set.id === ds)
      ?.channels.find((channel) => channel.id === ch)?.settings;

  const applyEdit = (ds: number, ch: number, edit: ChannelEdit): void => {
    // A refetch started by an earlier StateChanged could resolve after this write and clobber
    // it — cancel in-flight fetches before touching the cache (TanStack optimistic contract).
    void queryClient.cancelQueries({ queryKey: STATE_KEY });
    const prev = queryClient.getQueryData<StateSnapshot>(STATE_KEY);
    const current = cachedChannel(ds, ch);
    if (!prev || !current) {
      return;
    }
    const settings = mergeChannelSettings(
      current,
      typeof edit === "function" ? edit(current) : edit,
    );
    queryClient.setQueryData<StateSnapshot>(STATE_KEY, {
      ...prev,
      device_sets: prev.device_sets.map((set) =>
        set.id === ds
          ? {
              ...set,
              channels: set.channels.map((channel) =>
                channel.id === ch ? { ...channel, settings } : channel,
              ),
            }
          : set,
      ),
    });
    patchMut.mutate({ ds, ch, settings });
  };

  return { applyEdit, cachedChannel };
}
