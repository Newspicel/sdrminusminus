import { describe, expect, it } from "vitest";
import {
  newest,
  parseChangelog,
  parseChangeset,
  RELEASES,
  releases,
  renderSummary,
} from "./changelog";

const CHANGELOG = `# Changelog

## 1.10.0 (2026-10-01)

### Features

- Faster DSP ([abc1234](https://github.com/Newspicel/sdrminusminus/commit/abc1234def))

  Filters and resampling up to 3x.

### Fixes

- RTL-SDR: keep gain after reconnect
- Airspy: \`bias tee\` sticks

## 1.9.0 (2026-09-01)

### Fixes

- Older fix
`;

describe("parseChangelog", () => {
  it("reads versions, dates and groups", () => {
    const [latest, older] = parseChangelog(CHANGELOG);
    expect(latest?.version).toBe("1.10.0");
    expect(latest?.date).toBe("2026-10-01");
    expect(latest?.groups.map((group) => group.heading)).toEqual(["Features", "Fixes"]);
    expect(latest?.groups[1]?.items).toHaveLength(2);
    expect(older?.version).toBe("1.9.0");
  });

  it("keeps continuation paragraphs and commit links", () => {
    const [feature] = parseChangelog(CHANGELOG)[0]?.groups[0]?.items ?? [];
    expect(feature).toBe(
      '<p>Faster DSP (<a href="https://github.com/Newspicel/sdrminusminus/commit/abc1234def">abc1234</a>)</p><p>Filters and resampling up to 3x.</p>',
    );
  });

  it("reads an empty changelog as no releases", () => {
    expect(parseChangelog("# Changelog\n")).toEqual([]);
  });
});

describe("parseChangeset", () => {
  it("reads the bump and summary", () => {
    expect(parseChangeset("---\nbump: minor\n---\n\nAirspy HF+: add preamp control.\n")).toEqual({
      bump: "minor",
      summary: "Airspy HF+: add preamp control.",
    });
  });

  it("skips files without a valid bump or summary", () => {
    expect(parseChangeset("# Changesets\n")).toBeUndefined();
    expect(parseChangeset("---\nbump: huge\n---\n\nText\n")).toBeUndefined();
    expect(parseChangeset("---\nbump: patch\n---\n\n")).toBeUndefined();
  });
});

describe("releases", () => {
  it("puts pending changesets first, grouped by bump", () => {
    const all = releases(CHANGELOG, [
      "---\nbump: patch\n---\n\nA fix\n",
      "---\nbump: major\n---\n\nA break\n",
    ]);
    expect(all[0]?.version).toBe("Next release");
    expect(all[0]?.date).toBeUndefined();
    expect(all[0]?.groups.map((group) => group.heading)).toEqual(["Breaking changes", "Fixes"]);
    expect(all[1]?.version).toBe("1.10.0");
  });

  it("has no next release without changesets", () => {
    expect(releases(CHANGELOG, [])[0]?.version).toBe("1.10.0");
  });
});

describe("newest", () => {
  it("takes the first items of the latest release", () => {
    const latest = newest(parseChangelog(CHANGELOG), 2);
    expect(latest?.version).toBe("1.10.0");
    expect(latest?.groups.flatMap((group) => group.items)).toHaveLength(2);
  });

  it("is empty without releases", () => {
    expect(newest([], 3)).toBeUndefined();
  });
});

describe("renderSummary", () => {
  it("escapes HTML and links only HTTPS", () => {
    expect(renderSummary("<b> [x](javascript:alert(1))")).toBe(
      "<p>&lt;b&gt; [x](javascript:alert(1))</p>",
    );
  });
});

describe("RELEASES", () => {
  it("parses the repository changelog and changesets", () => {
    for (const release of RELEASES) {
      expect(release.groups.length, release.version).toBeGreaterThan(0);
    }
  });
});
