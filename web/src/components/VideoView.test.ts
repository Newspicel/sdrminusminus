import { describe, expect, it } from "vitest";
import { videoSignalText } from "./VideoView";

describe("videoSignalText", () => {
  it("waits for the first frame", () => {
    expect(videoSignalText(null)).toBe("waiting for sync");
  });

  it("reads the picture size and says when sync is lost", () => {
    expect(videoSignalText({ width: 320, height: 256, live: true })).toBe("320 × 256");
    expect(videoSignalText({ width: 320, height: 256, live: false })).toBe("320 × 256 · no sync");
  });
});
