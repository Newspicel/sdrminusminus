import { useMutation } from "@tanstack/react-query";
import { tuneArray as sendArrayTune } from "./api";
import { clearAction, failAction } from "./refusals";

export const TUNE_ACTION = "Tune";

interface ArrayTune {
  node: string;
  hz: number;
}

export function useArrayTune(): {
  tuneArray: (node: string, hz: number) => void;
  pending: boolean;
} {
  const tune = useMutation({
    mutationFn: ({ node, hz }: ArrayTune) => sendArrayTune(node, { center_hz: hz }),
    onSuccess: (_done, { node }) => clearAction(node, TUNE_ACTION),
    onError: (error, { node }) => failAction(node, TUNE_ACTION, error),
  });
  return {
    tuneArray: (node, hz) => tune.mutate({ node, hz }),
    pending: tune.isPending,
  };
}
