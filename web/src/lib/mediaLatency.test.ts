import { expect, it } from "vitest";
import { mediaLatencyMs, registerMediaLatency } from "./mediaLatency";

it("uses the active audio output's bounded latency and survives replacement", () => {
  let delay = 85;
  const old = registerMediaLatency("1:2", () => delay);
  expect(mediaLatencyMs("1:2")).toBe(85);
  expect(mediaLatencyMs("1:3")).toBe(0);
  delay = 900;
  expect(mediaLatencyMs("1:2")).toBe(500);
  const next = registerMediaLatency("1:2", () => 40);
  old();
  expect(mediaLatencyMs("1:2")).toBe(40);
  next();
  expect(mediaLatencyMs("1:2")).toBe(0);
});
