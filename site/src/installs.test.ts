import { describe, expect, it } from "vitest";
import { INSTALLS, installFor } from "./installs";

describe("installFor", () => {
  it("picks the package manager of the platform", () => {
    expect(installFor("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("container");
    expect(installFor("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5)")).toBe("brew");
    expect(installFor("Mozilla/5.0 (X11; Linux x86_64)")).toBe("apt");
    expect(installFor("Mozilla/5.0 (X11; Fedora; Linux x86_64)")).toBe("dnf");
  });

  it("falls back to Homebrew on phones", () => {
    expect(installFor("Mozilla/5.0 (Linux; Android 15; Pixel 9)")).toBe("brew");
  });

  it("only names installs that exist", () => {
    const ids = new Set(INSTALLS.map((install) => install.id));
    for (const agent of ["Windows", "Macintosh", "X11", "Fedora", "Android"]) {
      expect(ids.has(installFor(agent))).toBe(true);
    }
  });
});

describe("INSTALLS", () => {
  it("keeps shell line continuations", () => {
    const apt = INSTALLS.find((install) => install.id === "apt");
    expect(apt?.lines).toContain("key.gpg \\\n");
  });
});
