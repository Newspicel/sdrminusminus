import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Chips, ChoiceChip, NumberChip, ReadoutChip, ToggleChip } from "./Chips";
import { FaceFault } from "./Fault";
import { MeterRow } from "./Meter";
import { Readout, Readouts } from "./Readouts";
import { FaceStats, Stat } from "./Stats";

const noop = () => undefined;

describe("chips", () => {
  it("shows a choice by its option label", () => {
    const html = renderToStaticMarkup(
      <ChoiceChip
        label="Baud"
        title="POCSAG baud"
        value="b1200"
        options={[
          { value: "auto", label: "Auto" },
          { value: "b1200", label: "1200" },
        ]}
        onChange={noop}
      />,
    );
    expect(html).toContain("Baud");
    expect(html).toContain("1200");
    expect(html).toContain('aria-label="POCSAG baud"');
  });

  it("falls back to the raw value when no option matches", () => {
    const html = renderToStaticMarkup(
      <ChoiceChip label="Tone" title="Tone" value={71.9} options={[]} onChange={noop} />,
    );
    expect(html).toContain("71.9");
  });

  it("shows a number with its unit", () => {
    const html = renderToStaticMarkup(
      <NumberChip label="Margin" title="Margin" value={10} unit="dB" onCommit={noop} />,
    );
    expect(html).toContain("<b");
    expect(html).toContain("10");
    expect(html).toContain("dB");
  });

  it("marks a toggle pressed and spells its state", () => {
    const on = renderToStaticMarkup(
      <ToggleChip label="Invert" title="Flip polarity" on onChange={noop} />,
    );
    expect(on).toContain('aria-pressed="true"');
    expect(on).toContain(">on<");
    const off = renderToStaticMarkup(
      <ToggleChip label="Invert" title="Flip polarity" on={false} onChange={noop} />,
    );
    expect(off).toContain('aria-pressed="false"');
    expect(off).toContain(">off<");
  });

  it("tones a read-only chip", () => {
    const html = renderToStaticMarkup(
      <Chips>
        <ReadoutChip label="Rate" value="2 MS/s" title="Fixed" tone="warn" />
      </Chips>,
    );
    expect(html).toContain("text-warn");
    expect(html).toContain('title="Fixed"');
  });
});

describe("readouts", () => {
  it("lays cells out in the asked columns with a tone", () => {
    const html = renderToStaticMarkup(
      <Readouts columns={3}>
        <Readout label="Load" tone="danger">
          120 %
        </Readout>
      </Readouts>,
    );
    expect(html).toContain("grid-cols-3");
    expect(html).toContain("text-danger");
    expect(html).toContain("<dt");
  });

  it("drops the rule and padding on request", () => {
    const html = renderToStaticMarkup(
      <Readouts ruled={false} padded={false}>
        <Readout label="Age">1 s</Readout>
      </Readouts>,
    );
    expect(html).not.toContain("border-t");
    expect(html).not.toMatch(/[" ]p-2[" ]/);
  });
});

describe("stats", () => {
  it("tones the value, or the label when there is no value", () => {
    const valued = renderToStaticMarkup(
      <FaceStats>
        <Stat label="Drops" title="Dropped" tone="warn">
          3
        </Stat>
      </FaceStats>,
    );
    expect(valued).toMatch(/<b class="[^"]*text-warn/);
    const bare = renderToStaticMarkup(<Stat label="No ref" title="No reference" tone="danger" />);
    expect(bare).toMatch(/<span class="[^"]*text-danger/);
    expect(bare).not.toContain("<b");
  });
});

describe("fault", () => {
  it("shows a lone message without a fold", () => {
    const html = renderToStaticMarkup(<FaceFault message="Radio gone" />);
    expect(html).toContain('role="alert"');
    expect(html).toContain("Radio gone");
    expect(html).not.toContain("aria-expanded");
  });

  it("folds the raw detail under the message", () => {
    const html = renderToStaticMarkup(<FaceFault message="Radio gone" detail="EIO -5" />);
    expect(html).toContain("aria-expanded");
  });
});

describe("meter row", () => {
  it("renders label, readout and an empty trailing slot", () => {
    const html = renderToStaticMarkup(
      <MeterRow label="Squelch" meter={<span>meter</span>} readout="-40 dB" />,
    );
    expect(html).toContain("Squelch");
    expect(html).toContain("-40 dB");
    expect(html).toContain("grid-cols-[3.5rem_minmax(0,1fr)_3.75rem_2.75rem_0]");
  });
});
