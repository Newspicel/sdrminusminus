import { useEffect, useRef, useState } from "react";
import { type Colormap, DEFAULT_COLORMAP } from "../../gl/colormap";
import { attachWaterfall, type WaterfallView } from "../../gl/waterfall";
import { monitorKey, watchAudio } from "../../lib/audio/monitor";
import { AudioSpectrogram, audioNyquistHz } from "../../lib/dsp/audioSpectrum";

const TICKS_HZ = [3000, 6000, 12_000, 18_000];

export function AudioSpectrogramView({
  deviceSet,
  channel,
  playing,
  colormap = DEFAULT_COLORMAP,
}: {
  deviceSet: number;
  channel: number;
  playing: boolean;
  colormap?: Colormap;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallView | null>(null);
  const [error, setError] = useState<string | null>(null);
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
      if (!announced) {
        announced = true;
        setDrawing(true);
      }
    });
    return () => {
      stop();
      setDrawing(false);
    };
  }, [deviceSet, channel, playing]);

  return (
    <div className="relative h-24 w-full overflow-hidden rounded-[3px] bg-plot-bg">
      <canvas ref={canvasRef} className="h-full w-full" />

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
