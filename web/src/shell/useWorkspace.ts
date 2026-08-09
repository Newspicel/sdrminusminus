// The active workspace as the shell sees it: the list for the switcher, the active layout, and
// the mutations that write it back (PLAN §10, M6). Server state lives in TanStack Query only —
// WS `StateChanged { workspaces }` invalidates, nothing polls.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import {
  activateWorkspace,
  createWorkspace,
  deleteWorkspace,
  updateWorkspace,
  WORKSPACES_KEY,
  workspaceQuery,
  workspacesQuery,
} from "../lib/api";
import type { WorkspaceDetail, WorkspaceInfo, WorkspaceSnapshot } from "../lib/types";

export interface WorkspaceState {
  workspaces: WorkspaceInfo[];
  active: WorkspaceDetail | null;
  /** A layout write that is in flight or failed — surfaced rather than swallowed, since a
   * rejected write means the arrangement on screen is not the one that is stored. */
  error: string | null;
  saveSnapshot: (snapshot: WorkspaceSnapshot) => void;
  rename: (name: string) => void;
  activate: (id: number) => void;
  create: (name: string) => void;
  remove: (id: number) => void;
}

export function useWorkspace(): WorkspaceState {
  const queryClient = useQueryClient();
  const list = useQuery(workspacesQuery());
  const activeId = list.data?.active ?? null;
  const detail = useQuery(workspaceQuery(activeId));

  const update = useMutation({
    mutationFn: (variables: {
      id: number;
      revision: number;
      snapshot?: WorkspaceSnapshot;
      name?: string;
    }) =>
      updateWorkspace(variables.id, {
        revision: variables.revision,
        ...(variables.snapshot ? { snapshot: variables.snapshot } : {}),
        ...(variables.name ? { name: variables.name } : {}),
      }),
    // A 409 means another client wrote first. The fix is always the same — take their layout —
    // so refetching is the whole recovery, and the dock re-applies what came back.
    onSettled: () => queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY }),
  });
  const activateMut = useMutation({
    mutationFn: activateWorkspace,
    onSettled: () => queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY }),
  });
  const createMut = useMutation({
    mutationFn: (name: string) => createWorkspace(name),
    onSuccess: (id) => activateMut.mutate(id),
    onSettled: () => queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY }),
  });
  const removeMut = useMutation({
    mutationFn: deleteWorkspace,
    onSettled: () => queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY }),
  });

  const active = detail.data ?? null;
  const saveSnapshot = useCallback(
    (snapshot: WorkspaceSnapshot) => {
      if (active === null) {
        return;
      }
      // Show the new arrangement immediately: the round trip is a refetch away, and letting the
      // tab strip flicker back to the old layout for a frame is worse than a stale revision,
      // which the server would reject anyway.
      queryClient.setQueryData<WorkspaceDetail>([...WORKSPACES_KEY, active.id], (previous) =>
        previous ? { ...previous, snapshot } : previous,
      );
      update.mutate({ id: active.id, revision: active.revision, snapshot });
    },
    [active, queryClient, update],
  );

  const rename = useCallback(
    (name: string) => {
      if (active !== null && name.trim() !== "") {
        update.mutate({ id: active.id, revision: active.revision, name: name.trim() });
      }
    },
    [active, update],
  );

  return {
    workspaces: list.data?.workspaces ?? [],
    active,
    error: errorOf(update.error) ?? errorOf(createMut.error) ?? errorOf(removeMut.error),
    saveSnapshot,
    rename,
    activate: activateMut.mutate,
    create: createMut.mutate,
    remove: removeMut.mutate,
  };
}

function errorOf(error: Error | null): string | null {
  return error === null ? null : error.message;
}
