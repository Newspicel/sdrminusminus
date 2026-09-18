import { expect, it } from "vitest";
import { JitterBuffer } from "./jitter";
import { MAX_FRAMES, SAMPLE_RATE, targetFramesForHost } from "./worklet";

it("uses explicit local and remote audio latency budgets", () => {
  expect(targetFramesForHost("127.0.0.1") / SAMPLE_RATE).toBe(0.08);
  expect(targetFramesForHost("tauri.localhost") / SAMPLE_RATE).toBe(0.08);
  expect(targetFramesForHost("radio.example") / SAMPLE_RATE).toBe(0.1);
  expect(MAX_FRAMES / SAMPLE_RATE).toBe(0.4);
});

it.each([-2_700, 2_700])("handles packet jitter with %i ppm output clock drift", (ppm) => {
  const jitter = new JitterBuffer(targetFramesForHost("127.0.0.1"), MAX_FRAMES, 1);
  const outputs = [new Float32Array(128)];
  const packet = new Float32Array(960).fill(0.5);
  let rendered = 0;
  for (let index = 0; index < 9_000; index++) {
    const delay = index % 101 === 100 ? 0.035 * SAMPLE_RATE : 0;
    const deadline = Math.floor(((index * packet.length + delay) * (1 + ppm / 1e6)) / 128) * 128;
    while (rendered < deadline) {
      jitter.read(outputs);
      rendered += 128;
    }
    jitter.push(packet);
  }
  expect(jitter.underruns).toBe(0);
  expect(jitter.trimmed).toBe(0);
});
