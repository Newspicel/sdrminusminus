// The live picture from a video channel (PLAN §13: ATV), rendered as the lower half of that
// channel's node face — the same place a decoder's output goes (CANVAS §8 phase ③).
//
// Pictures bypass React state entirely and go straight to the canvas (PLAN §10: high-rate streams
// never touch TanStack Query). Only the readout — geometry and whether anything is arriving at
// all — is state, and it changes at a human rate rather than a field rate.
import { useEffect, useRef, useState } from "react";
import type { VideoFrame } from "../lib/frame";
import { videoHub } from "../lib/video";

/** A raster is transmitted for a 4:3 screen, and what a channel samples out of it is however many
 * pixels its bandwidth resolved along the line — far fewer than its 576 rows. So the canvas is
 * drawn at the picture's own size and *displayed* at the shape it was scanned for; stretching in
 * CSS is what keeps the source honest. */
const DISPLAY_ASPECT = 4 / 3;

/** No picture for this long and the readout says so. Longer than the gap between fields by a wide
 * margin, so a momentary loss of sync does not flicker the label. */
const STALE_MS = 2_000;

interface Geometry {
  width: number;
  height: number;
}

/** Which channel's pictures to draw. A concrete pair, unlike a decoder view's `DecoderScope`,
 * which is a filter over a shared store: a video stream is subscribed to, and there is no such
 * thing as subscribing to "every channel". */
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
    // `willReadFrequently` is deliberately absent: this context is written every field and never
    // read back, which is the case the GPU-backed path is for.
    const ctx = canvas.getContext("2d");
    if (ctx === null) {
      return;
    }
    // Reused across frames: an ImageData per field would be a 60 kB allocation fifty times a
    // second, and the geometry only changes when the standard does.
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
      for (let i = 0; i < frame.luma.length; i += 1) {
        const luma = frame.luma[i] ?? 0;
        const at = i * 4;
        rgba[at] = luma;
        rgba[at + 1] = luma;
        rgba[at + 2] = luma;
        rgba[at + 3] = 255;
      }
      ctx.putImageData(image, 0, 0);
      setLive(true);
      clearTimeout(stale);
      stale = window.setTimeout(() => setLive(false), STALE_MS);
    };

    // The last picture this channel sent, so a face that has just remounted opens on the raster
    // it was showing rather than on a blank canvas for a field period.
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
