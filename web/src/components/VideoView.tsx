import { useEffect, useRef, useState } from "react";
import type { VideoFrame } from "../lib/frame";
import { videoHub } from "../lib/video";

const DISPLAY_ASPECT = 4 / 3;

const STALE_MS = 2_000;

interface Geometry {
  width: number;
  height: number;
}

export interface VideoScope {
  deviceSet: number;
  channel: number;
}

export function VideoView({ scope }: { scope: VideoScope }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [geometry, setGeometry] = useState<Geometry | null>(null);
  const [live, setLive] = useState(false);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null) {
      return;
    }
    const ctx = canvas.getContext("2d");
    if (ctx === null) {
      return;
    }
    let image: ImageData | null = null;
    let stale = 0;

    const draw = (frame: VideoFrame): void => {
      if (image === null || image.width !== frame.width || image.height !== frame.height) {
        canvas.width = frame.width;
        canvas.height = frame.height;
        image = ctx.createImageData(frame.width, frame.height);
        setGeometry({ width: frame.width, height: frame.height });
      }
      const rgba = image.data;
      const count = frame.width * frame.height;
      for (let i = 0; i < count; i += 1) {
        const at = i * 4;
        if (frame.format === "rgb") {
          rgba[at] = frame.pixels[i * 3] ?? 0;
          rgba[at + 1] = frame.pixels[i * 3 + 1] ?? 0;
          rgba[at + 2] = frame.pixels[i * 3 + 2] ?? 0;
        } else {
          const luma = frame.pixels[i] ?? 0;
          rgba[at] = luma;
          rgba[at + 1] = luma;
          rgba[at + 2] = luma;
        }
        rgba[at + 3] = 255;
      }
      ctx.putImageData(image, 0, 0);
      setLive(true);
      clearTimeout(stale);
      stale = window.setTimeout(() => setLive(false), STALE_MS);
    };

    const held = videoHub.latest(scope.deviceSet, scope.channel);
    if (held !== null) {
      draw(held);
    }
    const stop = videoHub.subscribe(scope.deviceSet, scope.channel, draw);
    return () => {
      stop();
      clearTimeout(stale);
    };
  }, [scope.deviceSet, scope.channel]);

  return (
    <div className="flex flex-col gap-1 p-2">
      <div
        className="w-full overflow-hidden rounded-xs bg-black"
        style={{ aspectRatio: DISPLAY_ASPECT }}
      >
        <canvas
          ref={canvasRef}
          aria-label="Decoded video"
          className="h-full w-full"
          style={{ imageRendering: "pixelated" }}
        />
      </div>
      <p className="legend text-ink-faint">
        {geometry === null
          ? "waiting for sync"
          : `${geometry.width} × ${geometry.height}${live ? "" : " · no sync"}`}
      </p>
    </div>
  );
}
