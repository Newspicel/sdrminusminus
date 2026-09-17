import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { DevOnly } from "./DevOnly";

describe("DevOnly", () => {
  it("renders its children in a dev build", () => {
    vi.stubEnv("DEV", true);
    expect(renderToStaticMarkup(<DevOnly>Drops 12</DevOnly>)).toContain("Drops 12");
    vi.unstubAllEnvs();
  });

  it("renders nothing in a release build", () => {
    vi.stubEnv("DEV", false);
    expect(renderToStaticMarkup(<DevOnly>Drops 12</DevOnly>)).toBe("");
    vi.unstubAllEnvs();
  });
});
