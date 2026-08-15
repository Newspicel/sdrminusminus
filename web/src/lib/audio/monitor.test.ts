import { afterEach, describe, expect, it } from "vitest";
import { isWatched, monitorKey, publishAudio, resetAudioMonitor, watchAudio } from "./monitor";

afterEach(() => {
  resetAudioMonitor();
});

describe("watchAudio", () => {
  it("delivers a block to every watcher of that channel", () => {
    const first: number[][] = [];
    const second: number[][] = [];
    watchAudio("1:7", (pcm) => first.push([...pcm]));
    watchAudio("1:7", (pcm) => second.push([...pcm]));

    publishAudio("1:7", Float32Array.of(0.5, -0.5), 1);
    expect(first).toEqual([[0.5, -0.5]]);
    expect(second).toEqual([[0.5, -0.5]]);
  });

  it("carries the layout the block was decoded in", () => {
    let layout = 0;
    watchAudio("1:7", (_pcm, channels) => {
      layout = channels;
    });
    publishAudio("1:7", Float32Array.of(0, 0, 0, 0), 2);
    expect(layout).toBe(2);
  });

  it("keeps channels apart", () => {
    const seen: number[] = [];
    watchAudio("1:7", () => seen.push(7));
    watchAudio("1:8", () => seen.push(8));
    publishAudio("1:8", Float32Array.of(0), 1);
    expect(seen).toEqual([8]);
  });

  it("stops delivering once the watcher lets go", () => {
    let blocks = 0;
    const stop = watchAudio("1:7", () => {
      blocks += 1;
    });
    publishAudio("1:7", Float32Array.of(0), 1);
    stop();
    publishAudio("1:7", Float32Array.of(0), 1);
    expect(blocks).toBe(1);
  });

  it("publishing to a channel nobody watches is a no-op, not a throw", () => {
    expect(() => publishAudio("9:9", Float32Array.of(0), 1)).not.toThrow();
  });
});

describe("isWatched", () => {
  it("is the sink's cheap check before it publishes anything", () => {
    expect(isWatched("1:7")).toBe(false);
    const stop = watchAudio("1:7", () => {});
    expect(isWatched("1:7")).toBe(true);
    stop();
    expect(isWatched("1:7")).toBe(false);
  });

  it("stays true while any watcher is left", () => {
    const first = watchAudio("1:7", () => {});
    watchAudio("1:7", () => {});
    first();
    expect(isWatched("1:7")).toBe(true);
  });
});

describe("monitorKey", () => {
  it("matches the audio engine's own entry key", () => {
    expect(monitorKey(1, 7)).toBe("1:7");
  });
});
