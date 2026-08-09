// Spectrum line + WebGL2 waterfall for one device set. Binary frames bypass React state and go
// straight to the canvases (PLAN §10: high-rate streams never touch TanStack Query).
import { useEffect, useRef, useState } from "react";
import { WaterfallRenderer } from "../gl/waterfall";
import type { SpectrumFrame } from "../lib/frame";
import type { SdrSocket } from "../lib/ws";

const BINS = 1024;
const FPS = 30;

interface FrameMeta {
  centerHz: number;
  spanHz: number;
  dbMin: number;
  dbMax: number;
  seq: number;
}

export function SpectrumDisplay({
  socket,
  deviceSet,
  connected,
}: {
  socket: SdrSocket;
  deviceSet: number | null;
  connected: boolean;
}) {
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const lineRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallRenderer | null>(null);
  const [meta, setMeta] = useState<FrameMeta | null>(null);

  useEffect(() => {
    const canvas = waterfallRef.current;
    if (!canvas) {
      return;
    }
    const renderer = new WaterfallRenderer(canvas);
    rendererRef.current = renderer;
    return () => {
      renderer.dispose();
      rendererRef.current = null;
    };
  }, []);

  useEffect(() => {
    let frameCount = 0;
    socket.onSpectrum = (frame: SpectrumFrame) => {
      rendererRef.current?.pushRow(frame.bins);
      drawSpectrumLine(lineRef.current, frame);
      // Throttle React meta updates to ~4 Hz; the canvases update every frame.
      frameCount += 1;
      if (frameCount % 8 === 0) {
        setMeta({
          centerHz: frame.centerHz,
          spanHz: frame.spanHz,
          dbMin: frame.dbMin,
          dbMax: frame.dbMax,
          seq: frame.seq,
        });
      }
    };
    return () => {
      socket.onSpectrum = () => {};
    };
  }, [socket]);

  // Subscribe only once the socket is actually open, and re-subscribe whenever it reconnects
  // (`connected` cycles false→true). `send()` drops commands while not OPEN, so gating on
  // `connected` avoids both the initial CONNECTING race and a permanently frozen stream after a
  // reconnect (the new server connection has no subscriptions).
  useEffect(() => {
    if (deviceSet === null || !connected) {
      return;
    }
    socket.send({
      type: "SubscribeSpectrum",
      data: { device_set: deviceSet, fps: FPS, bins: BINS },
    });
    return () => {
      socket.send({ type: "UnsubscribeSpectrum", data: { device_set: deviceSet } });
    };
  }, [socket, deviceSet, connected]);

  // Canvases stay mounted even with no device so the WaterfallRenderer (created in the mount
  // effect above) always has a real canvas to attach to; the prompt overlays on top.
  return (
    <div className="relative flex flex-1 flex-col overflow-hidden">
      <canvas ref={lineRef} className="h-32 w-full shrink-0 bg-panel" />
      <canvas ref={waterfallRef} className="w-full flex-1 bg-bg" />
      {deviceSet === null ? (
        <div className="absolute inset-0 flex items-center justify-center text-ink-dim">
          Open a device to see the spectrum.
        </div>
      ) : (
        meta && (
          <div className="pointer-events-none absolute top-2 right-3 font-mono text-xs tabular-nums text-ink-dim">
            {(meta.centerHz / 1e6).toFixed(3)} MHz · {(meta.spanHz / 1e6).toFixed(3)} MHz span ·{" "}
            {meta.dbMin.toFixed(0)}…{meta.dbMax.toFixed(0)} dB
          </div>
        )
      )}
    </div>
  );
}

function drawSpectrumLine(canvas: HTMLCanvasElement | null, frame: SpectrumFrame): void {
  if (!canvas) {
    return;
  }
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = Math.round(canvas.clientWidth * dpr);
  const h = Math.round(canvas.clientHeight * dpr);
  if (w === 0 || h === 0) {
    return;
  }
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return;
  }

  ctx.clearRect(0, 0, w, h);
  ctx.strokeStyle = "rgba(125,138,160,0.14)";
  ctx.lineWidth = 1;
  for (let i = 1; i < 4; i++) {
    const y = (h * i) / 4;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }

  const bins = frame.bins;
  const n = bins.length;
  if (n < 2) {
    return;
  }
  ctx.strokeStyle = "#21b0b0";
  ctx.lineWidth = 1.25;
  ctx.beginPath();
  for (let i = 0; i < n; i++) {
    const x = (i / (n - 1)) * w;
    const y = (1 - (bins[i] ?? 0) / 255) * h;
    if (i === 0) {
      ctx.moveTo(x, y);
    } else {
      ctx.lineTo(x, y);
    }
  }
  ctx.stroke();
}
