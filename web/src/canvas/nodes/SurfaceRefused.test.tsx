import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { ListenerRegistry } from "../../lib/listeners";
import { surfaceHub } from "../../lib/surface";
import type { SurfaceRefusal } from "../../lib/types";
import { FusionHeat } from "./FusionHeat";
import { RadarPlot } from "./RadarPlot";

const stops: (() => void)[] = [];

function refuse(node: string, reason: SurfaceRefusal): void {
  const registry = new ListenerRegistry();
  surfaceHub.attach({
    send: () => {},
    isConnected: () => true,
    on: (kind, listener) => registry.on(kind, listener),
  });
  stops.push(surfaceHub.subscribe(node, () => {}));
  registry.emit("event", { type: "SurfaceRefused", data: { node, reason } });
}

afterEach(() => {
  for (const stop of stops.splice(0)) {
    stop();
  }
  surfaceHub.detach();
});

function heat(): string {
  return renderToStaticMarkup(
    <FusionHeat node="tri" known estimate={null} emitters={[]} stations={[]} hint={null} />,
  );
}

describe("a refused surface", () => {
  it("shows its reason on the heat instead of the empty hint", () => {
    expect(heat()).toContain("No bearings yet");
    refuse("tri", "no_surface");
    const html = heat();
    expect(html).toContain("No surface");
    expect(html).not.toContain("No bearings yet");
  });

  it("shows its reason on the radar plot", () => {
    refuse("radar", "no_stream_ids");
    const html = renderToStaticMarkup(
      <RadarPlot node="radar" known update={null} dim={false} colormap="classic" selected={null} />,
    );
    expect(html).toContain('role="status"');
    expect(html).toContain("Too many streams");
  });
});
