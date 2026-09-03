import { describe, expect, it } from "vitest";

const SOURCES: Record<string, string> = import.meta.glob("../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
});

const RETIRED = /[✕✎⧉↶↷▶■❚↻▾●✓○]/;
const IMPORT = /import \{([^}]*)\} from "lucide-react";/;

function rendered(text: string): string[] {
  const names = IMPORT.exec(text)?.[1] ?? "";
  return names
    .split(",")
    .map((name) => name.replace("type ", "").trim())
    .filter((name) => name !== "" && new RegExp(`<${name}[\\s/>]`).test(text));
}

describe("icons", () => {
  it("reach the page through the Icon wrapper, never as a bare lucide element", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([path]) => !path.endsWith("/Icon.tsx"))
      .filter(([, text]) => rendered(text).length > 0)
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });

  it("are never typed as characters", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([, text]) => RETIRED.test(text))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
