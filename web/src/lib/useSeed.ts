import type { UseQueryResult } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { clearAction, flagAction } from "./refusals";

export function useSeed<T>(
  node: string,
  action: string,
  query: UseQueryResult<T>,
  held: boolean,
  apply: (data: T) => void,
): void {
  const data = query.data;
  const error = query.error;
  const applied = useRef<T | undefined>(undefined);
  useEffect(() => {
    if (data === undefined || (held && applied.current === data)) {
      return;
    }
    applied.current = data;
    apply(data);
    clearAction(node, action);
  }, [node, action, data, held, apply]);
  useEffect(() => {
    if (error !== null) {
      flagAction(node, action, error);
    }
  }, [node, action, error]);
}
