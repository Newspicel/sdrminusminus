// A voiceprint of what a channel is playing, under its transport controls.
//
// Drawn from the audio the browser has already decoded for playback, so it costs one transform per
// hop and no extra stream — and it follows that it only runs while the channel is *playing*. That
// is the honest behaviour for a monitor of what you are hearing, and the empty state says so.
//
// The waterfall is the same renderer the spectrum scope uses: rows of bytes over a fixed dB
// window, scrolled on the GPU. Only the axis differs, and it is the one thing drawn here.

import { useEffect, useRef, useState } from "react";
import { type Colormap, DEFAULT_COLORMAP } from "../../gl/colormap";
import { attachWaterfall, type WaterfallView } from "../../gl/waterfall";
import { monitorKey, watchAudio } from "../../lib/audio/monitor";
import { AudioSpectrogram, audioNyquistHz } from "../../lib/dsp/audioSpectrum";

/** Marks on the frequency axis, which runs left to right like the spectrum waterfall's — a row
 * is the transform's bins. Four is what a node-width panel has room for. */
const TICKS_HZ = [3000, 6000, 12_000, 18_000];

export function AudioSpectrogramView({
  deviceSet,
  channel,
  playing,
  colormap = DEFAULT_COLORMAP,
}: {
  deviceSet: number;
  channel: number;
  /** Whether the channel is playing. Nothing is decoded when it is not, so nothing arrives. */
  playing: boolean;
  colormap?: Colormap;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallView | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Whether a row has been drawn since playback started — the difference between "waiting for
   * audio" and "not playing". */
  const [drawing, setDrawing] = useState(false);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null) {
      return;
    }
    let renderer: WaterfallView;
    try {
      renderer = attachWaterfall(canvas, setError);
    } catch (thrown) {
      // No WebGL2. The transport above still works, so the face keeps everything but this.
      setError(thrown instanceof Error ? thrown.message : String(thrown));
      return;
    }
    renderer.setColormap(colormap);
    rendererRef.current = renderer;
    return () => {
      renderer.dispose();
      rendererRef.current = null;
    };
  }, [colormap]);

  useEffect(() => {
    if (!playing) {
      return;
    }
    const spectrogram = new AudioSpectrogram();
    let announced = false;
    const stop = watchAudio(monitorKey(deviceSet, channel), (pcm, channels) => {
      spectrogram.push(pcm, channels, (row) => {
        rendererRef.current?.pushRow(row);
      });
      // Published once rather than per row: it only decides whether the "waiting" line still
      // stands, and a row arrives every ten milliseconds.
      if (!announced) {
        announced = true;
        setDrawing(true);
      }
    });
    return () => {
      stop();
      // A channel that has stopped playing leaves a frozen picture, which is a lie about a live
      // radio; the overlay covers it rather than the panel showing the last thing heard forever.
      setDrawing(false);
    };
  }, [deviceSet, channel, playing]);

  return (
    <div className="relative h-24 w-full overflow-hidden rounded-[3px] bg-plot-bg">
      <canvas ref={canvasRef} className="h-full w-full" />

      {/* Drawn over the plot rather than beside it: a node is narrow, and a gutter wide enough
          for "24k" would cost the picture a tenth of its width. Each label carries its own scrim
          of the plot ground — a spectrogram is bright wherever there is signal, and a label
          cannot borrow contrast from whatever happens to be under it. */}
      {TICKS_HZ.map((hz) => (
        <span
          key={hz}
          className="pointer-events-none absolute bottom-0.5 legend -translate-x-1/2 rounded-[2px] bg-plot-bg/75 px-0.5 leading-none text-plot-ink"
          style={{ left: `${(hz / audioNyquistHz()) * 100}%` }}
        >
          {`${(hz / 1000).toFixed(0)}k`}
        </span>
      ))}

      {error !== null && (
        <span className="pointer-events-none absolute inset-0 flex items-center justify-center px-2 text-center legend text-danger">
          {error}
        </span>
      )}
      {error === null && (!playing || !drawing) && (
        <span className="pointer-events-none absolute inset-0 flex items-center justify-center px-2 text-center legend text-plot-ink-dim">
          {playing
            ? "Waiting for audio…"
            : `Play this channel to see its audio up to ${(audioNyquistHz() / 1000).toFixed(0)} kHz.`}
        </span>
      )}
    </div>
  );
}
