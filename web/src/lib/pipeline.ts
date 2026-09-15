import { create } from "zustand";
import type { ServerEvent } from "./types";

type Health = Extract<ServerEvent, { type: "PipelineHealth" }>["data"];
export const usePipelineHealth = create<{
  health: Health | null;
  observe: (event: ServerEvent) => void;
  reset: () => void;
}>((set) => ({
  health: null,
  observe: (event) => {
    if (event.type === "PipelineHealth") set({ health: event.data });
  },
  reset: () => set({ health: null }),
}));

export function queueSummary(
  health: Health | null,
  deviceSet: number,
): { oldestMs: number; detail: string } | null {
  const queues = health?.queues.filter((queue) => queue.device_set === deviceSet) ?? [];
  if (queues.length === 0) return null;
  return {
    oldestMs: Math.max(...queues.map((queue) => queue.health.oldest_ms)),
    detail: queues
      .map(
        (queue) =>
          `${queue.stage} ${queue.stream}${queue.channel === null ? "" : `/${queue.channel}`}: ${queue.health.queued}/${queue.health.capacity}, ${queue.health.oldest_ms.toFixed(1)} ms, ${queue.health.dropped} dropped`,
      )
      .join("\n"),
  };
}
