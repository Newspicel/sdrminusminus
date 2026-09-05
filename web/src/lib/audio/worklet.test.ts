import { expect, it } from "vitest";
import { MAX_FRAMES, SAMPLE_RATE, targetFramesForHost } from "./worklet";

it("uses explicit local and remote audio latency budgets", () => {
  expect(targetFramesForHost("127.0.0.1") / SAMPLE_RATE).toBe(0.06);
  expect(targetFramesForHost("tauri.localhost") / SAMPLE_RATE).toBe(0.06);
  expect(targetFramesForHost("radio.example") / SAMPLE_RATE).toBe(0.1);
  expect(MAX_FRAMES / SAMPLE_RATE).toBe(0.4);
});
