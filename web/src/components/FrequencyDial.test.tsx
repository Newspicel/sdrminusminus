import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DirectEntry } from "./FrequencyDial";

function entry(height: number): string {
  return renderToStaticMarkup(
    <DirectEntry draft="" height={height} onDraft={() => {}} onCommit={() => {}} />,
  );
}

describe("DirectEntry", () => {
  it("takes the height of the dial it replaces so the node does not jump", () => {
    expect(entry(34)).toContain("height:34px");
    expect(entry(40)).toContain("height:40px");
  });

  it("keeps its own height when the dial was never measured", () => {
    expect(entry(0)).not.toContain("height:");
  });
});
