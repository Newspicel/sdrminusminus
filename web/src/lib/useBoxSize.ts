import { type RefObject, useEffect, useRef, useState } from "react";

export interface BoxSize {
  width: number;
  height: number;
}

export function useBoxSize<T extends Element>(): [RefObject<T | null>, BoxSize] {
  const ref = useRef<T | null>(null);
  const [size, setSize] = useState<BoxSize>({ width: 0, height: 0 });
  useEffect(() => {
    const element = ref.current;
    if (element === null) {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const box = entries.at(-1)?.contentRect;
      if (box === undefined) {
        return;
      }
      setSize((previous) =>
        previous.width === box.width && previous.height === box.height
          ? previous
          : { width: box.width, height: box.height },
      );
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, size];
}
