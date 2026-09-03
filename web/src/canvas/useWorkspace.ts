import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  activateWorkspace,
  applyWorkspace,
  cloneWorkspace,
  createWorkspace,
  deleteWorkspace,
  importWorkspace,
  STATE_KEY,
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
  WorkspacesResponse,
} from "../lib/types";
import { pruneRack } from "./graph";
import { WorkspaceDrafts } from "./workspaceDrafts";
import { parseWorkspaceExport } from "./workspaceExport";

export interface WorkspaceStore {
  workspaces: WorkspaceInfo[];
  active: WorkspaceDetail | null;
  error: string | null;
  save: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  activate: (id: number) => void;
  create: (name: string) => void;
  rename: (id: number, name: string) => void;
  clone: (id: number) => void;
  importFile: (file: File) => void;
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
  const revisionOf = useCallback(
    (id: number): number =>
      drafts.get(id)?.revision ??
      queryClient.getQueryData<WorkspaceDetail>([...WORKSPACES_KEY, id])?.revision ??
      queryClient
        .getQueryData<WorkspacesResponse>(WORKSPACES_KEY)
        ?.workspaces.find((entry) => entry.id === id)?.revision ??
      0,
    [drafts, queryClient],
  );
  const renameMut = useMutation({
    mutationFn: (variables: { id: number; name: string }) =>
      updateWorkspace(variables.id, { revision: revisionOf(variables.id), name: variables.name }),
    onSuccess: (info, variables) => {
      drafts.accepted(variables.id, info.revision);
      queryClient.setQueryData<WorkspaceDetail>([...WORKSPACES_KEY, variables.id], (previous) =>
        previous ? { ...previous, ...info } : previous,
      );
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY }),
  });
  const cloneMut = useMutation({
    mutationFn: cloneWorkspace,
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
  const importMut = useMutation({
    mutationFn: async (file: File) => importWorkspace(parseWorkspaceExport(await file.text())),
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
    onSuccess: (stepped, variables) => {
      queryClient.setQueryData<WorkspaceDetail>([...WORKSPACES_KEY, variables.id], stepped);
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
    },
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

  const renameAsync = renameMut.mutateAsync;
  const rename = useCallback(
    (id: number, name: string) => {
      const task = queue.current
        .catch(() => undefined)
        .then(() => renameAsync({ id, name }))
        .catch(() => undefined);
      finishQueue(task);
    },
    [finishQueue, renameAsync],
  );

  const cloneAsync = cloneMut.mutateAsync;
  const clone = useCallback(
    (id: number) => {
      const task = queue.current
        .catch(() => undefined)
        .then(() => cloneAsync(id))
        .catch(() => undefined);
      finishQueue(task);
    },
    [cloneAsync, finishQueue],
  );

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
      errorOf(renameMut.error) ??
      errorOf(cloneMut.error) ??
      errorOf(importMut.error) ??
      errorOf(removeMut.error) ??
      errorOf(applyMut.error) ??
      errorOf(stepMut.error),
    save,
    activate: activateMut.mutate,
    create: createMut.mutate,
    rename,
    clone,
    importFile: importMut.mutate,
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
