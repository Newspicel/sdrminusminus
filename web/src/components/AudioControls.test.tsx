import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AudioControls } from "./AudioControls";

describe("AudioControls", () => {
  it("renders every audio stage as a chip", () => {
    const html = renderToStaticMarkup(<AudioControls audio={{}} onAudio={() => undefined} />);
    for (const label of ["Audio AGC speed", "Click removal", "Noise reduction", "Audio filter"]) {
      expect(html).toContain(`aria-label="${label}"`);
    }
  });

  it("shows one chip per notch", () => {
    const html = renderToStaticMarkup(
      <AudioControls
        audio={{ notches: [{ freq_hz: 1_000, width_hz: 100 }, { freq_hz: 2_000 }] }}
        onAudio={() => undefined}
      />,
    );
    expect(html).toContain('aria-label="Notch 1"');
    expect(html).toContain('aria-label="Notch 2"');
    expect(html).toContain("2 kHz");
  });

  it("shows the passband when the filter is on", () => {
    const html = renderToStaticMarkup(
      <AudioControls audio={{ filter: { enabled: true } }} onAudio={() => undefined} />,
    );
    expect(html).toContain("300 Hz – 3 kHz");
  });
});
