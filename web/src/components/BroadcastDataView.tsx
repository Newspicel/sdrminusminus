import { useMemo } from "react";
import type { DecoderEvent } from "../lib/types";

type Data = Extract<DecoderEvent, { kind: "broadcast_data" }>["data"];

export function BroadcastDataView({ data }: { data: Data }) {
  const image = ["image/jpeg", "image/png", "image/gif", "image/bmp"].includes(data.media_type);
  const url = useMemo(() => {
    const encoded = btoa(data.bytes.map((byte) => String.fromCharCode(byte)).join(""));
    const type = image ? data.media_type : "application/octet-stream";
    return `data:${type};base64,${encoded}`;
  }, [data, image]);
  const name = Array.from(data.name, (char) =>
    char.charCodeAt(0) < 32 || char === "/" || char === "\\" ? "_" : char,
  ).join("");
  return (
    <div className="flex flex-col items-start gap-2">
      {image && <img src={url} alt={data.name} className="max-h-96 max-w-full object-contain" />}
      <a href={url} download={name} className="text-accent underline">
        Download {data.name}
      </a>
    </div>
  );
}
