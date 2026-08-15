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
    onError: (error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const cachedChannel = (ds: number, ch: number): ChannelSettings | undefined =>
    queryClient
      .getQueryData<StateSnapshot>(STATE_KEY)
      ?.device_sets.find((set) => set.id === ds)
      ?.channels.find((channel) => channel.id === ch)?.settings;

  const applyEdit = (ds: number, ch: number, edit: ChannelEdit): void => {
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
