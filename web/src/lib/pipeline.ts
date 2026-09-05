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
