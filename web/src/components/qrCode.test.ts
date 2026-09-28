import { encode } from "uqr";
import { describe, expect, it } from "vitest";
import { QR_BORDER, qrPath } from "./QrCode";

const PAIRING_URI =
  "sdrmm://pair?h=192.168.1.20:8443&h=shack.local:8443&c=48210937" +
  "&fp=3fa9c2e07b1d4a6e9f0c2b8d5e7a1c3f9b2d4e6a8c0e2f4a6b8d0c2e4f6a8b0c&p=1";

describe("qrPath", () => {
  it("draws one square per dark module", () => {
    const { data, size } = encode("hello", { ecc: "M", border: QR_BORDER });
    const dark = data.flatMap((row, y) => row.flatMap((on, x) => (on ? [`${x} ${y}`] : [])));
    const drawn = qrPath("hello");
    expect(drawn.size).toBe(size);
    expect(drawn.path.match(/h1v1/g)).toHaveLength(dark.length);
    const squares = [...drawn.path.matchAll(/M(\d+) (\d+)h1v1h-1z/g)].map(
      ([, x, y]) => `${x} ${y}`,
    );
    expect(squares).toEqual(dark);
  });

  it("encodes the pairing URI", () => {
    expect(PAIRING_URI.length).toBeGreaterThan(120);
    const { size, path } = qrPath(PAIRING_URI);
    expect(size).toBeGreaterThanOrEqual(25);
    expect(path.startsWith(`M${QR_BORDER} ${QR_BORDER}h1v1h-1z`)).toBe(true);
  });
});
