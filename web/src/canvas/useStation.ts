// The active station as the canvas sees it: the workspace list for the switcher, the active
// patch, and the writes that persist it (PLAN §10, CANVAS §4). Server state lives in TanStack
// Query only — WS `StateChanged { workspaces }` invalidates, nothing polls.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef } from "react";
import {
  activateWorkspace,
  applyWorkspace,
  createWorkspace,
  deleteWorkspace,
  updateWorkspace,
  WORKSPACES_KEY,
  workspaceQuery,
  workspacesQuery,
} from "../lib/api";
import type {
  PatchApplyReport,
  WorkspaceDetail,
  WorkspaceInfo,
  WorkspaceSnapshot,
} from "../lib/types";

export interface StationState {
  workspaces: WorkspaceInfo[];
  active: WorkspaceDetail | null;
  /** A write that failed — surfaced rather than swallowed, since a rejected write means the
   * patch on screen is not the one that is stored. */
  error: string | null;
  /** Edit the active station. The edit is a *function* of the current snapshot, not a snapshot:
   * two changes can land within one round trip (a node drag and the wire it ended on), and the
   * second must build on the first rather than on what the caller happened to be rendering. */
  save: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  activate: (id: number) => void;
  create: (name: string) => void;
  remove: (id: number) => void;
  /** Bring the engine up to the patch (CANVAS §2). Additive and idempotent server-side. */
  apply: () => void;
  applied: PatchApplyReport | null;
  /** The list has not answered yet — distinct from "there are no stations", which is a real
   * state the server reports after the last one is deleted. */
  pending: boolean;
}

export function useStation(): StationState {
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
    // A 409 means another client wrote first. The fix is always the same — take their patch —
    // so refetching is the whole recovery, and the canvas re-applies what came back.
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
  const applyMut = useMutation({ mutationFn: applyWorkspace });
  const applyAsync = applyMut.mutateAsync;

  const active = detail.data ?? null;
  const activeIdRef = useRef<number | null>(null);
  activeIdRef.current = active?.id ?? null;
  // Writes are serialized: each one reads the revision the previous one produced. Issuing them
  // concurrently would send the same revision twice, and the server — correctly — refuses the
  // second as stale, which would silently drop whichever change lost the race.
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  const save = useCallback(
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
          // the canvas flicker back to the old patch for a frame is worse than a brief
          // divergence the very next response settles.
          queryClient.setQueryData<WorkspaceDetail>(key, { ...current, snapshot });
          return update.mutateAsync({ id, revision: current.revision, snapshot });
        })
        // The failure is already on screen through `update.error`; this only keeps the last one
        // in a chain from surfacing as an unhandled rejection.
        .catch(() => undefined);
    },
    [queryClient, update],
  );

  // Apply goes through the same queue as a write, and that ordering is load-bearing: the gesture
  // that draws a wire saves the patch and then asks for it to be applied, and an apply that
  // overtook the write would bring the engine up to the *previous* graph — the new channel would
  // silently never be created.
  const apply = useCallback(() => {
    const id = activeIdRef.current;
    if (id === null) {
      return;
    }
    queue.current = queue.current
      .catch(() => undefined)
      .then(() => applyAsync(id))
      .catch(() => undefined);
  }, [applyAsync]);

  // Applying is idempotent, so it runs once per station that becomes active: opening the app on
  // a station whose radios are attached should give you the station, not an empty canvas waiting
  // to be clicked into life.
  //
  // Keyed on the *loaded* station, not on the id the list reports: `apply` reads the id off the
  // detail query, which is still resolving on the render the list first names one, so an effect
  // keyed on that id would fire once into a no-op and mark itself done.
  const applied = useRef<number | null>(null);
  const loaded = active?.id ?? null;
  useEffect(() => {
    if (loaded !== null && applied.current !== loaded) {
      applied.current = loaded;
      apply();
    }
  }, [loaded, apply]);

  return {
    workspaces: list.data?.workspaces ?? [],
    active,
    error:
      errorOf(update.error) ??
      errorOf(createMut.error) ??
      errorOf(removeMut.error) ??
      errorOf(applyMut.error),
    save,
    activate: activateMut.mutate,
    create: createMut.mutate,
    remove: removeMut.mutate,
    apply,
    applied: applyMut.data ?? null,
    pending: list.isPending || (activeId !== null && detail.isPending),
  };
}

function errorOf(error: Error | null): string | null {
  return error === null ? null : error.message;
}
