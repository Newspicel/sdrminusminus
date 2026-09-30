import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { FUSION_KEY, SURVEY_KEY } from "./api";
import { useArrayStore } from "./arrays";
import { forgetNodes, resetNodeState } from "./nodeState";
import { surfaceHub } from "./surface";
import type { ArrayStatus, PatchNode } from "./types";

const NODE_QUERIES = [FUSION_KEY, SURVEY_KEY] as const;

export function switchedWorkspace(last: number | null, next: number | null): boolean {
  return last !== null && next !== null && last !== next;
}

export function retryNodeState(queryClient: QueryClient): void {
  for (const queryKey of NODE_QUERIES) {
    void queryClient.refetchQueries({
      queryKey,
      predicate: (query) => query.state.status === "error",
    });
  }
  surfaceHub.retry();
}

export function goneNodes(drawn: ReadonlySet<string>, nodes: readonly PatchNode[]): string[] {
  const ids = new Set(nodes.map((node) => node.id));
  return [...drawn].filter((id) => !ids.has(id));
}

export function useNodeStateSync(
  workspace: number | null,
  nodes: readonly PatchNode[],
  arrays: readonly ArrayStatus[] | undefined,
): void {
  const queryClient = useQueryClient();
  const shown = useRef<number | null>(null);
  useEffect(() => {
    if (switchedWorkspace(shown.current, workspace)) {
      resetNodeState();
      for (const queryKey of NODE_QUERIES) {
        void queryClient.invalidateQueries({ queryKey });
      }
    }
    if (workspace !== null) {
      shown.current = workspace;
    }
    useArrayStore.getState().seed(arrays ?? []);
  }, [workspace, arrays, queryClient]);

  const drawn = useRef<ReadonlySet<string>>(new Set());
  useEffect(() => {
    forgetNodes(goneNodes(drawn.current, nodes));
    drawn.current = new Set(nodes.map((node) => node.id));
  }, [nodes]);
}
