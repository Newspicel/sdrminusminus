import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  findDirectBaseUiImports,
  findUnwrappedControls,
  isInsideDirectory,
} from "./shadcn-guard.mjs";

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

  it("does not exempt a sibling whose name starts with ui", () => {
    const components = join("src", "components");
    const ui = join(components, "ui");
    expect(isInsideDirectory(join(ui, "button.tsx"), ui)).toBe(true);
    expect(isInsideDirectory(join(components, "ui-legacy", "button.tsx"), ui)).toBe(false);
  });
});
