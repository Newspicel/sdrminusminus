import { afterEach, expect, it } from "vitest";
import { queueSummary, usePipelineHealth } from "./pipeline";

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

it("summarizes the queues of one device set and leaves the rest alone", () => {
  const health = {
    queues: [
      {
        device_set: 1,
        stream: 0,
        channel: null,
        stage: "capture" as const,
        health: { queued: 2400, capacity: 240000, oldest_ms: 1.25, dropped: 5 },
      },
      {
        device_set: 1,
        stream: 0,
        channel: 3,
        stage: "spectrum" as const,
        health: { queued: 8, capacity: 16, oldest_ms: 4.5, dropped: 0 },
      },
      {
        device_set: 2,
        stream: 0,
        channel: null,
        stage: "capture" as const,
        health: { queued: 1, capacity: 16, oldest_ms: 99, dropped: 0 },
      },
    ],
    websocket: { queued: 0, capacity: 16777216, oldest_ms: 0, dropped: 0 },
  };
  const summary = queueSummary(health, 1);
  expect(summary?.oldestMs).toBe(4.5);
  expect(summary?.detail).toBe(
    "capture 0: 2400/240000, 1.3 ms, 5 dropped\nspectrum 0/3: 8/16, 4.5 ms, 0 dropped",
  );
  expect(queueSummary(health, 7)).toBeNull();
  expect(queueSummary(null, 1)).toBeNull();
});
