import { useEffect, useState } from "react";
import { attachGrid, type Colormap, type GridFrame, type GridView } from "../gl/surface";
import { useBoxSize } from "../lib/useBoxSize";

export function GridCanvas({
  frame,
  colormap,
  flipY,
  transparentFloor = false,
  className,
}: {
  frame: GridFrame | null;
  colormap: Colormap;
  flipY: boolean;
  transparentFloor?: boolean;
  className?: string;
}) {
  const [ref, size] = useBoxSize<HTMLCanvasElement>();
  const [view, setView] = useState<GridView | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (canvas === null) {
      return;
    }
    const attached = attachGrid(canvas, { flipY, transparentFloor });
    setView(attached);
    return () => {
      attached.dispose();
      setView(null);
    };
  }, [ref, flipY, transparentFloor]);

  useEffect(() => {
    if (view === null || frame === null || size.width === 0 || size.height === 0) {
      return;
    }
    view.setColormap(colormap);
    view.draw(frame);
  }, [view, frame, colormap, size.width, size.height]);

  return <canvas ref={ref} aria-hidden className={className ?? "absolute inset-0 size-full"} />;
}
