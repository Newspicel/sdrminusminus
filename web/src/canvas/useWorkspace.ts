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
import { pruneRack } from "./graph";
import { WorkspaceDrafts } from "./workspaceDrafts";

export interface WorkspaceStore {
  workspaces: WorkspaceInfo[];
  active: WorkspaceDetail | null;
  /** A write that failed — surfaced rather than swallowed, since a rejected write means the
   * patch on screen is not the one that is stored. */
  error: string | null;
  /** Edit the active workspace. The edit is a *function* of the current snapshot, not a snapshot:
   * two changes can land within one round trip (a node drag and the wire it ended on), and the
   * second must build on the first rather than on what the caller happened to be rendering. */
  save: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  activate: (id: number) => void;
  create: (name: string) => void;
  remove: (id: number) => void;
  apply: () => void;
  applied: PatchApplyReport | null;
  /** The list has not answered yet — distinct from "there are no workspaces", which is a real
   * state the server reports after the last one is deleted. */
  pending: boolean;
}

export function useWorkspace(): WorkspaceStore {
  const queryClient = useQueryClient();
  const list = useQuery(workspacesQuery());
  const activeId = list.data?.active ?? null;
  const detail = useQuery(workspaceQuery(activeId));

  // A refetch is allowed to replace the query cache while writes are queued (the server emits a
  // workspace invalidation for every accepted write). Keep the composed local draft outside that
  // cache until the last queued write settles, or a refetch containing only an earlier edit can
  // erase a later one before it has been sent.
  // Keyed by workspace because a switch can put A, B and then A back into the same global write
  // queue. Settling one workspace must neither discard another's draft nor retain A's old revision.
  const drafts = useRef(new WorkspaceDrafts());

  const update = useMutation({
    mutationFn: (variables: { id: number; revision: number; snapshot: WorkspaceSnapshot }) =>
      updateWorkspace(variables.id, {
        revision: variables.revision,
        snapshot: variables.snapshot,
      }),
    // The write's own answer carries the new revision, and the next queued write reads it from
    // here. Preserve the latest local draft at the same time: the server's answer describes this
    // write, which may not be the last edit already waiting behind it.
    onSuccess: (info, variables) => {
      const snapshot = drafts.current.accepted(variables.id, info.revision) ?? variables.snapshot;
      queryClient.setQueryData<WorkspaceDetail>([...WORKSPACES_KEY, variables.id], (previous) =>
        previous
          ? {
              ...previous,
              ...info,
              snapshot,
            }
          : previous,
      );
    },
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

  const queried = detail.data ?? null;
  const draft = queried === null ? undefined : drafts.current.get(queried.id);
  const active =
    queried !== null && draft !== undefined ? { ...queried, snapshot: draft.snapshot } : queried;
  const activeIdRef = useRef<number | null>(null);
  activeIdRef.current = active?.id ?? null;
  // Writes are serialized: each one reads the revision the previous one produced. Issuing them
  // concurrently would send the same revision twice, and the server — correctly — refuses the
  // second as stale, which would silently drop whichever change lost the race.
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const refreshOwed = useRef(false);
  const finishQueue = useCallback(
    (task: Promise<unknown>) => {
      queue.current = task;
      // Only the actual tail pays for the authoritative refetch. `apply` uses this same finalizer,
      // so a save followed by its apply cannot strand the refresh merely because the apply is last.
      void task
        .then(() => {
          if (queue.current === task && refreshOwed.current) {
            refreshOwed.current = false;
            return queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY });
          }
        })
        .catch(() => undefined);
    },
    [queryClient],
  );

  const save = useCallback(
    (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => {
      const id = activeIdRef.current;
      if (id === null) {
        return;
      }
      const key = [...WORKSPACES_KEY, id] as const;
      const current = queryClient.getQueryData<WorkspaceDetail>(key);
      if (current === undefined) {
        return;
      }
      // Compose against the pending draft, not necessarily the query cache. A StateChanged
      // refetch may have replaced the cache with the last snapshot the server accepted while a
      // newer local edit is still queued.
      const base = drafts.current.get(id)?.snapshot ?? current.snapshot;
      const snapshot = fitRack(edit(base));
      const write = drafts.current.stage(id, snapshot, current.revision);
      // Applied to the cache *synchronously*, in the same task as the gesture that ended: a
      // drag's own preview is dropped on pointer-up, and anything that renders the stored
      // arrangement between the two — one microtask, or one whole round trip when a previous
      // write is still in flight — is a frame of the face back where it started. That frame is
      // the flicker. Reading the cache rather than a captured snapshot is what keeps two edits
      // within one round trip composing: the second sees the first.
      queryClient.setQueryData<WorkspaceDetail>(key, {
        ...current,
        snapshot,
      });
      // Capture this edit's composed snapshot now. Only the revision is read when its turn comes:
      // the cache may legitimately refetch before then, but that must not change what this write
      // means.
      const task = queue.current
        .catch(() => undefined)
        .then(async () => {
          try {
            const latest = queryClient.getQueryData<WorkspaceDetail>(key);
            if (latest !== undefined) {
              await update.mutateAsync({
                id,
                revision: drafts.current.get(id)?.revision ?? latest.revision,
                snapshot,
              });
            }
          } catch {
            // `update.error` owns the visible failure; the queue must still clean up and refetch.
          } finally {
            const finished = drafts.current.finish(id, write.generation);
            refreshOwed.current = refreshOwed.current || finished;
          }
        })
        .catch(() => undefined);
      finishQueue(task);
    },
    [finishQueue, queryClient, update],
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
    const task = queue.current
      .catch(() => undefined)
      .then(() => applyAsync(id))
      .catch(() => undefined);
    finishQueue(task);
  }, [applyAsync, finishQueue]);

  // Applying is idempotent, so it runs once per workspace that becomes active: opening the app on
  // a workspace whose radios are attached should give you the workspace, not an empty canvas waiting
  // to be clicked into life.
  //
  // Keyed on the *loaded* workspace, not on the id the list reports: `apply` reads the id off the
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

/** The snapshot with its rack re-laid out if it no longer fits the grid (`pruneRack`). */
function fitRack(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  const rack = pruneRack(snapshot.rack ?? {}, snapshot.graph);
  return rack === snapshot.rack ? snapshot : { ...snapshot, rack };
}
