import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Kbd, KbdGroup } from "./kbd";
import { Separator } from "./separator";
import { Slider } from "./slider";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./tabs";
import { ToggleGroup, ToggleGroupItem } from "./toggle-group";

function rangeInputs(markup: string): number {
  return markup.match(/<input\b[^>]*\btype="range"/g)?.length ?? 0;
}

function renderToggleGroup(orientation: "horizontal" | "vertical"): string {
  return renderToStaticMarkup(
    <ToggleGroup defaultValue={["one"]} orientation={orientation}>
      <ToggleGroupItem value="one">One</ToggleGroupItem>
    </ToggleGroup>,
  );
}

describe("shadcn Base UI primitives", () => {
  it("renders one labelled thumb for controlled and uncontrolled scalar sliders", () => {
    const controlled = renderToStaticMarkup(
      <Slider aria-label="Gain" value={12} min={0} max={20} />,
    );
    const uncontrolled = renderToStaticMarkup(
      <Slider aria-label="Volume" defaultValue={5} min={0} max={10} />,
    );

    expect(rangeInputs(controlled)).toBe(1);
    expect(controlled).toContain('aria-label="Gain"');
    expect(rangeInputs(uncontrolled)).toBe(1);
    expect(uncontrolled).toContain('aria-label="Volume"');
  });

  it("renders one indexed thumb for each range value", () => {
    const markup = renderToStaticMarkup(
      <Slider aria-label="Window" defaultValue={[20, 80]} min={0} max={100} />,
    );
    expect(rangeInputs(markup)).toBe(2);
    expect(markup).toContain('aria-label="Window"');
  });

  it("emits horizontal and vertical separator orientation", () => {
    const horizontal = renderToStaticMarkup(<Separator orientation="horizontal" />);
    const vertical = renderToStaticMarkup(<Separator orientation="vertical" />);
    expect(horizontal).toContain('data-orientation="horizontal"');
    expect(vertical).toContain('data-orientation="vertical"');
  });

  it("forwards vertical orientation to tabs", () => {
    const markup = renderToStaticMarkup(
      <Tabs defaultValue="one" orientation="vertical">
        <TabsList>
          <TabsTrigger value="one">One</TabsTrigger>
        </TabsList>
        <TabsContent value="one">Panel</TabsContent>
      </Tabs>,
    );
    expect(markup).toContain('data-orientation="vertical"');
  });

  it("emits orientation for horizontal and vertical toggle groups", () => {
    expect(renderToggleGroup("horizontal")).toContain('data-orientation="horizontal"');
    expect(renderToggleGroup("vertical")).toContain('data-orientation="vertical"');
  });

  it("groups keyboard keys in a generic container", () => {
    const markup = renderToStaticMarkup(
      <KbdGroup>
        <Kbd>Ctrl</Kbd>
        <Kbd>K</Kbd>
      </KbdGroup>,
    );
    expect(markup).toMatch(/^<div[^>]*data-slot="kbd-group"/);
  });
});
