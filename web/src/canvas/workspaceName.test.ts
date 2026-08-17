import { describe, expect, it } from "vitest";
import { MAX_NAME_LEN } from "./graph";
import { DEFAULT_WORKSPACE_NAME, workspaceName } from "./workspaceName";

describe("workspaceName", () => {
  it("keeps what was typed", () => {
    expect(workspaceName("Bench")).toBe("Bench");
  });

  it("trims the edges the server would reject", () => {
    expect(workspaceName("  Bench  ")).toBe("Bench");
  });

  it("falls back to the default when nothing was typed", () => {
    expect(workspaceName("")).toBe(DEFAULT_WORKSPACE_NAME);
    expect(workspaceName("   ")).toBe(DEFAULT_WORKSPACE_NAME);
  });

  it("cuts a name the server would refuse for its length", () => {
    expect(workspaceName("x".repeat(MAX_NAME_LEN + 10))).toBe("x".repeat(MAX_NAME_LEN));
  });
});
