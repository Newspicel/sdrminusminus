import type { UseQueryResult } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { clearAction, flagAction } from "./refusals";

export interface Seed<T> {
  data: T | undefined;
  at: number;
}

export function plantSeed<T>(
  planted: { current: Seed<T> | null },
  seed: Seed<T>,
  held: boolean,
  apply: (data: T) => void,
): boolean {
  const { data, at } = seed;
  const last = planted.current;
  if (data === undefined || (last !== null && last.data === data && last.at === at)) {
    return false;
  }
  planted.current = seed;
  if (!held) {
    apply(data);
  }
  return true;
}

export function useSeed<T>(
  node: string,
  action: string,
  query: UseQueryResult<T>,
  held: boolean,
  apply: (data: T) => void,
): void {
  const data = query.data;
  const at = query.dataUpdatedAt;
  const error = query.error;
  const planted = useRef<Seed<T> | null>(null);
  useEffect(() => {
    if (plantSeed(planted, { data, at }, held, apply)) {
      clearAction(node, action);
    }
  }, [node, action, data, at, held, apply]);
  useEffect(() => {
    if (error !== null && !held) {
      flagAction(node, action, error);
    }
  }, [node, action, error, held]);
}
