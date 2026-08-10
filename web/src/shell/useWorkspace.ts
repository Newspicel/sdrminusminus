// The active workspace as the shell sees it: the list for the switcher, the active layout, and
// the writes that persist it (PLAN §10, M6). Server state lives in TanStack Query only —
// WS `StateChanged { workspaces }` invalidates, nothing polls.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useRef } from "react";
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
  /** A write that failed — surfaced rather than swallowed, since a rejected write means the
   * arrangement on screen is not the one that is stored. */
  error: string | null;
  /** Edit the active workspace's layout. The edit is a *function* of the current snapshot, not a
   * snapshot: two changes can land within one round trip (a panel drag and the tab switch that
   * unmounts the dock), and the second must build on the first rather than on what the caller
   * happened to be rendering. */
  saveSnapshot: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
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
    mutationFn: (variables: { id: number; revision: number; snapshot: WorkspaceSnapshot }) =>
      updateWorkspace(variables.id, {
        revision: variables.revision,
        snapshot: variables.snapshot,
      }),
    // The write's own answer carries the new revision, and the next queued write reads it from
    // here: without folding it in, that write would still send the revision this one consumed
    // and the server would refuse it as stale.
    onSuccess: (info, variables) =>
      queryClient.setQueryData<WorkspaceDetail>([...WORKSPACES_KEY, variables.id], (previous) =>
        previous ? { ...previous, ...info } : previous,
      ),
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
  const activeIdRef = useRef<number | null>(null);
  activeIdRef.current = active?.id ?? null;
  // Writes are serialized: each one reads the revision the previous one produced. Issuing them
  // concurrently would send the same revision twice, and the server — correctly — refuses the
  // second as stale, which would silently drop whichever change lost the race.
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  const saveSnapshot = useCallback(
    (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => {
      const id = activeIdRef.current;
      if (id === null) {
        return;
      }
      const key = [...WORKSPACES_KEY, id] as const;
      queue.current = queue.current
        .catch(() => undefined)
        .then(() => {
          const current = queryClient.getQueryData<WorkspaceDetail>(key);
          if (current === undefined) {
            return undefined;
          }
          const snapshot = edit(current.snapshot);
          // Show the new arrangement immediately: the round trip is a refetch away, and letting
          // the shell flicker back to the old layout for a frame is worse than a brief
          // divergence the very next response settles.
          queryClient.setQueryData<WorkspaceDetail>(key, { ...current, snapshot });
          return update.mutateAsync({ id, revision: current.revision, snapshot });
        });
    },
    [queryClient, update],
  );

  return {
    workspaces: list.data?.workspaces ?? [],
    active,
    error: errorOf(update.error) ?? errorOf(createMut.error) ?? errorOf(removeMut.error),
    saveSnapshot,
    activate: activateMut.mutate,
    create: createMut.mutate,
    remove: removeMut.mutate,
  };
}

function errorOf(error: Error | null): string | null {
  return error === null ? null : error.message;
}
