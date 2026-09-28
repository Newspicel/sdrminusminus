import { encode } from "uqr";

export const QR_BORDER = 2;

const DEFAULT_SIZE = 192;

export function qrPath(value: string): { size: number; path: string } {
  const { data, size } = encode(value, { ecc: "M", border: QR_BORDER });
  let path = "";
  data.forEach((row, y) => {
    row.forEach((dark, x) => {
      if (dark) {
        path += `M${x} ${y}h1v1h-1z`;
      }
    });
  });
  return { size, path };
}

export function QrCode({
  value,
  label,
  size = DEFAULT_SIZE,
}: {
  value: string;
  label: string;
  size?: number;
}) {
  const qr = qrPath(value);
  return (
    <div className="self-center rounded bg-white p-2">
      <svg
        role="img"
        aria-label={label}
        width={size}
        height={size}
        viewBox={`0 0 ${qr.size} ${qr.size}`}
        shapeRendering="crispEdges"
      >
        <rect width={qr.size} height={qr.size} fill="#fff" />
        <path d={qr.path} fill="#000" />
      </svg>
    </div>
  );
}
