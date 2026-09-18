import { expect, it } from "vitest";
import { JitterBuffer } from "./jitter";
import { MAX_FRAMES, SAMPLE_RATE, TARGET_FRAMES } from "./worklet";

it("reserves delivery headroom within the audio latency budget", () => {
  expect(TARGET_FRAMES / SAMPLE_RATE).toBe(0.1);
  expect(MAX_FRAMES / SAMPLE_RATE).toBe(0.4);
});

it.each([-2_700, 2_700])("handles packet jitter with %i ppm output clock drift", (ppm) => {
  const jitter = new JitterBuffer(TARGET_FRAMES, MAX_FRAMES, 1);
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

it("absorbs consecutive delivery delays after the stream falls one packet behind", () => {
  const jitter = new JitterBuffer(TARGET_FRAMES, MAX_FRAMES, 1);
  const outputs = [new Float32Array(128)];
  const packet = new Float32Array(960).fill(0.5);
  let rendered = 0;
  for (let index = 0; index < 500; index++) {
    const deliveryTime = Math.ceil((index * 20) / 25) * 25;
    const streamDelay = index >= 110 ? 20 : 0;
    const packetDelay = index === 354 ? 30 : index === 355 ? 70 : 0;
    const deadline = Math.floor(((deliveryTime + streamDelay + packetDelay) * SAMPLE_RATE) / 1000);
    while (rendered + 128 <= deadline) {
      jitter.read(outputs);
      rendered += 128;
    }
    jitter.push(packet);
  }
  expect(jitter.underruns).toBe(0);
  expect(jitter.trimmed).toBe(0);
});
