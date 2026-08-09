// Spectrum line + WebGL2 waterfall for one device set. Binary frames bypass React state and go
// straight to the canvases (PLAN §10: high-rate streams never touch TanStack Query).
import { useEffect, useRef, useState } from "react";
import { WaterfallRenderer } from "../gl/waterfall";
import type { SpectrumFrame } from "../lib/frame";
import type { ChannelInfo } from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import { formatSignedKhz } from "./format";
import { markerFraction } from "./markers";

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
  channels,
  selectedChannel,
  onSelectChannel,
}: {
  socket: SdrSocket;
  deviceSet: number | null;
  connected: boolean;
  channels: ChannelInfo[];
  selectedChannel: number | null;
  onSelectChannel: (ch: number) => void;
}) {
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const lineRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallRenderer | null>(null);
  const [meta, setMeta] = useState<FrameMeta | null>(null);
  const [glError, setGlError] = useState<string | null>(null);

  useEffect(() => {
    const canvas = waterfallRef.current;
    if (!canvas) {
      return;
    }
    let renderer: WaterfallRenderer;
    try {
      renderer = new WaterfallRenderer(canvas);
    } catch (error) {
      // No WebGL2, a driver that refuses the shader, a lost context: the waterfall is the
      // centerpiece, but throwing out of a dock panel's mount takes the whole UI down with it.
      // The spectrum line, the controls and every other panel still work without it.
      setGlError(error instanceof Error ? error.message : String(error));
      return;
    }
    rendererRef.current = renderer;
    return () => {
      renderer.dispose();
      rendererRef.current = null;
    };
  }, []);

  useEffect(() => {
    // A device-set switch invalidates the cached meta: markers and the readout must never
    // be placed by the previous set's span/center — the new set may never emit a frame.
    setMeta(null);
    let frameCount = 0;
    socket.onSpectrum = (frame: SpectrumFrame) => {
      // Spectrum stream ids are device-set ids; drop late frames from a previous set.
      if (frame.streamId !== deviceSet) {
        return;
      }
      rendererRef.current?.pushRow(frame.bins);
      drawSpectrumLine(lineRef.current, frame);
      // Meta seeds from the first frame, then throttles to ~4 Hz; canvases update every frame.
      frameCount += 1;
      if (frameCount === 1 || frameCount % 8 === 0) {
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
  }, [socket, deviceSet]);

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
      {glError !== null && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 flex justify-center p-2">
          <span className="rounded border border-danger bg-bg/90 px-2 py-1 font-mono text-xs text-danger">
            waterfall unavailable: {glError}
          </span>
        </div>
      )}
      {deviceSet === null ? (
        <div className="absolute inset-0 flex items-center justify-center text-ink-dim">
          Open a device to see the spectrum.
        </div>
      ) : (
        meta && (
          <>
            <div className="pointer-events-none absolute top-2 right-3 font-mono text-xs tabular-nums text-ink-dim">
              {(meta.centerHz / 1e6).toFixed(3)} MHz · {(meta.spanHz / 1e6).toFixed(3)} MHz span ·{" "}
              {meta.dbMin.toFixed(0)}…{meta.dbMax.toFixed(0)} dB
            </div>
            {/* Markers must not steal pan/scroll gestures from the display: the layer is
                pointer-transparent, only each marker's own button takes clicks. */}
            <div className="pointer-events-none absolute inset-0 overflow-hidden">
              {channels.map((c) => {
                const offsetHz = c.settings.offset_hz ?? 0;
                const fraction = markerFraction(offsetHz, meta.spanHz);
                if (fraction === null) {
                  return null;
                }
                const active = c.id === selectedChannel;
                return (
                  <button
                    key={c.id}
                    type="button"
                    // `max-md:w-10` = the ≥40px phone touch target (controls.ts convention);
                    // the visible 1px line below is centered independently, so only the
                    // invisible hit area widens.
                    className="pointer-events-auto absolute inset-y-0 w-4 -translate-x-1/2 max-md:w-10"
                    style={{ left: `${fraction * 100}%` }}
                    onClick={() => onSelectChannel(c.id)}
                    aria-label={`Select ${c.settings.params.type} channel at ${formatSignedKhz(offsetHz)}`}
                  >
                    <span
                      className={`absolute inset-y-0 left-1/2 w-px ${
                        active ? "bg-accent" : "bg-ink-dim/60"
                      }`}
                    />
                    <span
                      className={`absolute top-1 left-1/2 -translate-x-1/2 whitespace-nowrap rounded-sm bg-bg/80 px-1 font-mono text-[10px] tabular-nums ${
                        active ? "text-accent" : "text-ink-dim"
                      }`}
                    >
                      {c.settings.params.type.toUpperCase()} {formatSignedKhz(offsetHz)}
                    </span>
                  </button>
                );
              })}
            </div>
          </>
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
