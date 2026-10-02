import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DENOISE_MODELS_KEY } from "../lib/api";
import type { AudioProcessing, DenoiseModelsResponse } from "../lib/types";
import { AudioControls } from "./AudioControls";

function render(audio: AudioProcessing, models?: DenoiseModelsResponse): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  if (models !== undefined) {
    client.setQueryData(DENOISE_MODELS_KEY, models);
  }
  return renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <AudioControls audio={audio} onAudio={() => undefined} />
    </QueryClientProvider>,
  );
}

const NEURAL: AudioProcessing = {
  denoise: { enabled: true, mode: "neural", model: "dpdfnet4", strength: 0.5 },
};

describe("AudioControls", () => {
  it("renders every audio stage as a chip", () => {
    const html = render({});
    for (const label of ["Audio AGC speed", "Click removal", "Noise reduction", "Audio filter"]) {
      expect(html).toContain(`aria-label="${label}"`);
    }
  });

  it("shows one chip per notch", () => {
    const html = render({ notches: [{ freq_hz: 1_000, width_hz: 100 }, { freq_hz: 2_000 }] });
    expect(html).toContain('aria-label="Notch 1"');
    expect(html).toContain('aria-label="Notch 2"');
    expect(html).toContain("2 kHz");
  });

  it("shows the passband when the filter is on", () => {
    const html = render({ filter: { enabled: true } });
    expect(html).toContain("300 Hz – 3 kHz");
  });

  it("flags a DPDFNet model that is not downloaded", () => {
    const html = render(NEURAL, {
      models: [{ model: "dpdfnet4", bytes: 5_817_413, state: "missing" }],
    });
    expect(html).toContain("no model");
  });

  it("shows the strength once the model is here", () => {
    const html = render(NEURAL, {
      models: [{ model: "dpdfnet4", bytes: 5_817_413, state: "ready" }],
    });
    expect(html).not.toContain("no model");
    expect(html).toContain("50%");
  });
});
