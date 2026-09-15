import { afterEach, describe, expect, it, vi } from "vitest";
import { copyText } from "./copyText";

function fakeDom(copied: boolean, focused: { focus: () => void } | null = null) {
  const field = {
    value: "",
    readOnly: false,
    style: {} as Record<string, string>,
    select: vi.fn(),
    remove: vi.fn(),
  };
  const document = {
    activeElement: focused,
    createElement: vi.fn(() => field),
    body: { append: vi.fn() },
    execCommand: vi.fn(() => copied),
  };
  return { field, document };
}

describe("copyText", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("writes through the clipboard API when the origin is granted one", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    const { document } = fakeDom(false);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    vi.stubGlobal("document", document);

    await copyText("124.925");

    expect(writeText).toHaveBeenCalledWith("124.925");
    expect(document.execCommand).not.toHaveBeenCalled();
  });

  it("copies by selection where a plain-HTTP origin withholds it", async () => {
    const { field, document } = fakeDom(true);
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("document", document);

    await copyText("124.925");

    expect(field.value).toBe("124.925");
    expect(field.select).toHaveBeenCalled();
    expect(document.execCommand).toHaveBeenCalledWith("copy");
    expect(field.remove).toHaveBeenCalled();
  });

  it("copies by selection when the clipboard API is there but refuses", async () => {
    const { document } = fakeDom(true);
    vi.stubGlobal("navigator", {
      clipboard: { writeText: () => Promise.reject(new Error("write permission denied")) },
    });
    vi.stubGlobal("document", document);

    await copyText("124.925");

    expect(document.execCommand).toHaveBeenCalledWith("copy");
  });

  it("says what copying needs when neither way is allowed", async () => {
    const { document } = fakeDom(false);
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("document", document);

    await expect(copyText("124.925")).rejects.toThrow(/HTTPS or a localhost address/);
  });

  it("hands focus back to whatever held it", async () => {
    const focus = vi.fn();
    const { document } = fakeDom(true, { focus });
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("document", document);

    await copyText("124.925");

    expect(focus).toHaveBeenCalled();
  });
});
