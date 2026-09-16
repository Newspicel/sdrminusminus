import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { BroadcastDataView } from "./BroadcastDataView";

describe("broadcast data", () => {
  it("shows a received slideshow and preserves its original bytes for download", () => {
    const html = renderToStaticMarkup(
      createElement(BroadcastDataView, {
        data: { name: "slide.png", media_type: "image/png", bytes: [137, 80, 78, 71] },
      }),
    );
    expect(html).toContain('alt="slide.png"');
    expect(html).toContain("data:image/png;base64,iVBORw==");
    expect(html).toContain('download="slide.png"');
  });

  it("downloads received HTML as data without embedding active content", () => {
    const html = renderToStaticMarkup(
      createElement(BroadcastDataView, {
        data: {
          name: "../page.html",
          media_type: "text/html",
          bytes: [60, 115, 99, 114, 105, 112, 116, 62],
        },
      }),
    );
    expect(html).toContain("data:application/octet-stream;base64,");
    expect(html).toContain('download=".._page.html"');
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<iframe");
  });
});
