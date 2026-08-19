import { useCallback, useEffect, useState } from "react";

interface WakeLock {
  release(): Promise<void>;
}

/// Keeps the screen on while a mission is open.
///
/// A phone that dims mid-drive is a phone the operator has to touch, and the whole point of the
/// compass and the click track is that they do not have to. The lock is taken again after every
/// visibility change, because a browser drops it whenever the tab goes away.
export function useWakeLock(active: boolean): void {
  useEffect(() => {
    if (!active) {
      return;
    }
    const api = (
      navigator as unknown as {
        wakeLock?: { request(kind: "screen"): Promise<WakeLock> };
      }
    ).wakeLock;
    if (api === undefined) {
      return;
    }
    let held: WakeLock | null = null;
    let stopped = false;
    const take = (): void => {
      if (stopped || document.visibilityState !== "visible") {
        return;
      }
      api
        .request("screen")
        .then((lock) => {
          held = lock;
        })
        .catch(() => {});
    };
    take();
    document.addEventListener("visibilitychange", take);
    return () => {
      stopped = true;
      document.removeEventListener("visibilitychange", take);
      void held?.release().catch(() => {});
    };
  }, [active]);
}

export function useFullscreen(): { full: boolean; toggle: () => void } {
  const [full, setFull] = useState(() => document.fullscreenElement !== null);
  useEffect(() => {
    const listener = (): void => setFull(document.fullscreenElement !== null);
    document.addEventListener("fullscreenchange", listener);
    return () => document.removeEventListener("fullscreenchange", listener);
  }, []);
  const toggle = useCallback(() => {
    if (document.fullscreenElement === null) {
      void document.documentElement.requestFullscreen?.().catch(() => {});
    } else {
      void document.exitFullscreen?.().catch(() => {});
    }
  }, []);
  return { full, toggle };
}

/// Voice guidance, armed by the first tap.
///
/// Mobile browsers refuse to speak until the page has been touched, so the mission arms this the
/// first time the operator interacts and says nothing before that rather than failing silently.
export function useVoice(): { armed: boolean; arm: () => void; say: (text: string) => void } {
  const [armed, setArmed] = useState(false);
  const arm = useCallback(() => {
    if (armed || typeof speechSynthesis === "undefined") {
      return;
    }
    speechSynthesis.cancel();
    setArmed(true);
  }, [armed]);
  const say = useCallback(
    (text: string) => {
      if (!armed || typeof speechSynthesis === "undefined" || text.length === 0) {
        return;
      }
      speechSynthesis.speak(new SpeechSynthesisUtterance(text));
    },
    [armed],
  );
  return { armed, arm, say };
}
