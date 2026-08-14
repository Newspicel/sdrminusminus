import { describe, expect, it } from "vitest";
import { findDirectBaseUiImports, findUnwrappedControls } from "./shadcn-guard.mjs";

describe("shadcn source guard", () => {
  it("rejects native controls that have shadcn equivalents", () => {
    expect(
      findUnwrappedControls("<button>Save</button><input /><select><option /></select>"),
    ).toEqual(["button", "input", "select", "option"]);
  });

  it("allows shadcn components and semantic forms", () => {
    expect(findUnwrappedControls("<form><Button>Save</Button><Input /></form>")).toEqual([]);
  });

  it("rejects direct Base UI imports in application components", () => {
    expect(findDirectBaseUiImports('import { Button } from "@base-ui/react/button";')).toEqual([
      'from "@base-ui/react/button"',
    ]);
  });
});
