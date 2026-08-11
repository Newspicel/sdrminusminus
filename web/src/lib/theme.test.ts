import { describe, expect, it } from "vitest";
import { nextTheme, THEME_CYCLE, type ThemeChoice } from "./theme";

describe("nextTheme", () => {
  it("walks every choice and comes back", () => {
    const walked: ThemeChoice[] = [];
    let choice: ThemeChoice = "system";
    for (let step = 0; step < THEME_CYCLE.length; step += 1) {
      walked.push(choice);
      choice = nextTheme(choice);
    }
    expect(walked).toEqual([...THEME_CYCLE]);
    expect(choice).toBe("system");
  });
});
