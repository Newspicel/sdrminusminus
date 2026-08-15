import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  activateWorkspace,
  applyWorkspace,
  createWorkspace,
  deleteWorkspace,
  stepWorkspace,
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
  error: string | null;
  save: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  activate: (id: number) => void;
  create: (name: string) => void;
  remove: (id: number) => void;
  apply: () => void;
  applied: PatchApplyReport | null;
  undo: () => void;
  redo: () => void;
  canUndo: boolean;
  canRedo: boolean;
  pending: boolean;
  unreachable: string | null;
}

export function useWorkspace(): WorkspaceStore {
  const queryClient = useQueryClient();
  const list = useQuery(workspacesQuery());
  const activeId = list.data?.active ?? null;
  const detail = useQuery(workspaceQuery(activeId));

  const [drafts] = useState(() => new WorkspaceDrafts());

  const update = useMutation({
    mutationFn: (variables: { id: number; revision: number; snapshot: WorkspaceSnapshot }) =>
      updateWorkspace(variables.id, {
        revision: variables.revision,
        snapshot: variables.snapshot,
      }),
    onSuccess: (info, variables) => {
      const snapshot = drafts.accepted(variables.id, info.revision) ?? variables.snapshot;
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
  const stepMut = useMutation({
    mutationFn: (variables: { id: number; step: "undo" | "redo" }) =>
      stepWorkspace(variables.id, variables.step),
    onSuccess: (stepped, variables) =>
      queryClient.setQueryData<WorkspaceDetail>([...WORKSPACES_KEY, variables.id], stepped),
  });
  const stepAsync = stepMut.mutateAsync;

  const queried = detail.data ?? null;
  const draft = queried === null ? undefined : drafts.get(queried.id);
  const active =
    queried !== null && draft !== undefined ? { ...queried, snapshot: draft.snapshot } : queried;
  const activeIdRef = useRef<number | null>(null);
  useLayoutEffect(() => {
    activeIdRef.current = active?.id ?? null;
  });
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const refreshOwed = useRef(false);
  const finishQueue = useCallback(
    (task: Promise<unknown>) => {
      queue.current = task;
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
      const base = drafts.get(id)?.snapshot ?? current.snapshot;
      const snapshot = fitRack(edit(base));
      const write = drafts.stage(id, snapshot, current.revision);
      queryClient.setQueryData<WorkspaceDetail>(key, {
        ...current,
        snapshot,
      });
      const task = queue.current
        .catch(() => undefined)
        .then(async () => {
          try {
            const latest = queryClient.getQueryData<WorkspaceDetail>(key);
            if (latest !== undefined) {
              await update.mutateAsync({
                id,
                revision: drafts.get(id)?.revision ?? latest.revision,
                snapshot,
              });
            }
          } catch {
          } finally {
            const finished = drafts.finish(id, write.generation);
            refreshOwed.current = refreshOwed.current || finished;
          }
        })
        .catch(() => undefined);
      finishQueue(task);
    },
    [drafts, finishQueue, queryClient, update],
  );

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

  const step = useCallback(
    (which: "undo" | "redo") => {
      const id = activeIdRef.current;
      if (id === null) {
        return;
      }
      const task = queue.current
        .catch(() => undefined)
        .then(() => stepAsync({ id, step: which }))
        .catch(() => undefined);
      finishQueue(task);
    },
    [finishQueue, stepAsync],
  );
  const undo = useCallback(() => step("undo"), [step]);
  const redo = useCallback(() => step("redo"), [step]);

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
      errorOf(applyMut.error) ??
      errorOf(stepMut.error),
    save,
    activate: activateMut.mutate,
    create: createMut.mutate,
    remove: removeMut.mutate,
    apply,
    applied: applyMut.data ?? null,
    undo,
    redo,
    canUndo: queried?.history?.can_undo ?? false,
    canRedo: queried?.history?.can_redo ?? false,
    pending: list.isPending || (activeId !== null && detail.isPending),
    unreachable: errorOf(list.error),
  };
}

function errorOf(error: Error | null): string | null {
  return error === null ? null : error.message;
}

function fitRack(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  const rack = pruneRack(snapshot.rack ?? {}, snapshot.graph);
  return rack === snapshot.rack ? snapshot : { ...snapshot, rack };
}
