import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { useRefusalStore, WIRE_REFUSAL_MS } from "../lib/refusals";
import { FaceAlert } from "./FaceAlert";

afterEach(() => useRefusalStore.getState().reset());

describe("FaceAlert", () => {
  it("renders nothing while the node has no refusal", () => {
    expect(renderToStaticMarkup(<FaceAlert node="arr" />)).toBe("");
  });

  it("shows the newest refusal in one line with its full text as the title", () => {
    useRefusalStore.getState().flag("arr", "that lane is in North", "action");
    const html = renderToStaticMarkup(<FaceAlert node="arr" />);
    expect(html).toContain('role="alert"');
    expect(html).toContain('title="that lane is in North"');
    expect(html).toContain('aria-label="Dismiss"');
  });

  it("hides a wire refusal once it has shown for six seconds", () => {
    useRefusalStore.setState({
      byNode: { arr: [{ reason: "old", source: "wire", at: Date.now() - WIRE_REFUSAL_MS - 50 }] },
    });
    expect(renderToStaticMarkup(<FaceAlert node="arr" />)).toBe("");
    useRefusalStore.getState().flag("arr", "that lane is in North", "wire");
    expect(renderToStaticMarkup(<FaceAlert node="arr" />)).toContain("that lane is in North");
  });
});
