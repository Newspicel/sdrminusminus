import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ChannelParams } from "../lib/types";
import { bandwidthOptions, ModeChips, serviceOptions } from "./ModeChips";

function render(params: ChannelParams): string {
  return renderToStaticMarkup(<ModeChips params={params} limits={[]} onParams={() => undefined} />);
}

describe("ModeChips", () => {
  it("shows the CTCSS chip only for CTCSS tone squelch", () => {
    expect(render({ type: "nfm", settings: {} })).not.toContain('aria-label="CTCSS tone"');
    const html = render({ type: "nfm", settings: { tone_mode: "ctcss", ctcss_hz: 100 } });
    expect(html).toContain('aria-label="CTCSS tone"');
    expect(html).toContain("100.0 Hz");
  });

  it("names the bandwidth chip for e2e", () => {
    expect(render({ type: "nfm", settings: {} })).toContain('aria-label="Channel bandwidth"');
  });

  it("swaps DVB-S code rate for S2 settings", () => {
    expect(render({ type: "datv", settings: {} })).toContain('aria-label="DVB-S code rate"');
    const s2 = render({ type: "datv", settings: { standard: "dvb_s2" } });
    expect(s2).not.toContain('aria-label="DVB-S code rate"');
    expect(s2).toContain("Roll-off");
  });

  it("offers Auto for the radiosonde type", () => {
    expect(render({ type: "radiosonde", settings: {} })).toContain("Auto");
    expect(render({ type: "radiosonde", settings: { sonde: "dfm" } })).toContain("DFM");
  });

  it("shows WEFAX timing and LRPT mode chips", () => {
    const wefax = render({ type: "wefax", settings: {} });
    expect(wefax).toContain('aria-label="Index of cooperation"');
    expect(wefax).toContain('aria-label="Lines per minute"');
    expect(render({ type: "lrpt", settings: {} })).toContain("OQPSK 72k");
  });

  it("shows the detected DAB mode next to Auto", () => {
    const params: ChannelParams = { type: "dab", settings: { transmission_mode: "auto" } };
    expect(render(params)).toContain("Auto");
    const html = renderToStaticMarkup(
      <ModeChips
        params={params}
        broadcast={{
          system: "dab",
          locked: true,
          snr_db: 12,
          frequency_error_hz: 0,
          transmission_mode: "ii",
        }}
        limits={[]}
        onParams={() => undefined}
      />,
    );
    expect(html).toContain("Auto II");
  });

  it("offers a service picker for DRM", () => {
    expect(render({ type: "drm", settings: {} })).toContain("Service");
  });

  it("renders nothing for modes without settings", () => {
    expect(render({ type: "m17", settings: {} })).toBe("");
  });
});

describe("bandwidthOptions", () => {
  it("keeps an off-list value as current", () => {
    const options = bandwidthOptions(15_000, [12_500, 25_000]);
    expect(options[0]).toEqual({ value: 15_000, label: "15 kHz (current)" });
    expect(options).toHaveLength(3);
  });

  it("matches numeric values", () => {
    expect(bandwidthOptions(12_500, [12_500, 25_000]).map((o) => o.value)).toEqual([
      12_500, 25_000,
    ]);
  });
});

describe("serviceOptions", () => {
  const status = [{ id: 7, label: "News" }];

  it("lists discovered services after Auto", () => {
    expect(serviceOptions(status, null)).toEqual([
      { value: "", label: "Auto" },
      { value: "7", label: "News" },
    ]);
  });

  it("keeps an unknown chosen service", () => {
    expect(serviceOptions(status, 9).at(-1)).toEqual({ value: "9", label: "9" });
  });
});
