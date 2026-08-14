import { describe, expect, it } from "vitest";
import { findNativeControls } from "./base-ui-guard.mjs";

describe("Base UI source guard", () => {
  it("rejects native interactive controls", () => {
    expect(
      findNativeControls("<button>Save</button><input /><select /><datalist><option /></datalist>"),
    ).toEqual(["button", "input", "select", "datalist", "option"]);
  });

  it("accepts Base UI components and semantic HTML", () => {
    expect(findNativeControls("<Button>Save</Button><Input /><label>Name</label>")).toEqual([]);
  });
});
