import { describe, expect, it } from "vitest";
import { fromGitHub, type GitHubRelease, newest, parseNotes, renderSummary } from "./changelog";

const NOTES = `### Features

- Faster DSP ([abc1234](https://github.com/Newspicel/sdrminusminus/commit/abc1234def))

  Filters and resampling up to 3x.

### Fixes

- RTL-SDR: keep gain after reconnect
- Airspy: \`bias tee\` sticks

Container image: \`docker pull ghcr.io/newspicel/sdrminusminus:1.10.0\`

## What's Changed
* Something by @someone

**Full Changelog**: https://github.com/Newspicel/sdrminusminus/compare/v1.9.0...v1.10.0
`;

function release(
  tag: string,
  body: string | null,
  extra: Partial<GitHubRelease> = {},
): GitHubRelease {
  return {
    tag_name: tag,
    published_at: "2026-10-01T12:00:00Z",
    draft: false,
    prerelease: false,
    body,
    ...extra,
  };
}

describe("parseNotes", () => {
  it("reads the changeset groups", () => {
    const groups = parseNotes(NOTES);
    expect(groups.map((group) => group.heading)).toEqual(["Features", "Fixes"]);
    expect(groups[1]?.items).toHaveLength(2);
  });

  it("keeps continuation paragraphs and commit links", () => {
    expect(parseNotes(NOTES)[0]?.items[0]).toBe(
      '<p>Faster DSP (<a href="https://github.com/Newspicel/sdrminusminus/commit/abc1234def">abc1234</a>)</p><p>Filters and resampling up to 3x.</p>',
    );
  });

  it("stops at text GitHub adds after the notes", () => {
    expect(parseNotes(NOTES)[1]?.items[1]).toBe("<p>Airspy: <code>bias tee</code> sticks</p>");
  });
});

describe("fromGitHub", () => {
  it("keeps only published stable releases with notes", () => {
    const all = fromGitHub([
      release("nightly", NOTES, { prerelease: true }),
      release("v1.11.0", NOTES, { draft: true, published_at: null }),
      release("v1.10.0", NOTES),
      release("v1.9.0", "Container image: `docker pull x`"),
    ]);
    expect(all.map((entry) => entry.version)).toEqual(["1.10.0"]);
    expect(all[0]?.date).toBe("2026-10-01");
  });
});

describe("newest", () => {
  it("takes the first items of the latest release", () => {
    const latest = newest(fromGitHub([release("v1.10.0", NOTES)]), 2);
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
