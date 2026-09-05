import { afterEach, expect, it } from "vitest";
import { usePipelineHealth } from "./pipeline";

afterEach(() => usePipelineHealth.getState().reset());
it("publishes stage metrics and clears stale metrics on disconnect", () => {
  usePipelineHealth.getState().observe({
    type: "PipelineHealth",
    data: {
      queues: [
        {
          device_set: 1,
          stream: 0,
          channel: null,
          stage: "capture",
          health: { queued: 2400, capacity: 240000, oldest_ms: 1, dropped: 5 },
        },
      ],
      websocket: { queued: 1024, capacity: 16777216, oldest_ms: 2, dropped: 3 },
    },
  });
  expect(usePipelineHealth.getState().health?.queues[0]?.health.dropped).toBe(5);
  usePipelineHealth.getState().reset();
  expect(usePipelineHealth.getState().health).toBeNull();
});
