export type PcmListener = (pcm: Float32Array, channels: number) => void;

const listeners = new Map<string, Set<PcmListener>>();

export function monitorKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

export function watchAudio(key: string, listener: PcmListener): () => void {
  const set = listeners.get(key) ?? new Set<PcmListener>();
  listeners.set(key, set);
  set.add(listener);
  return () => {
    set.delete(listener);
    if (set.size === 0) {
      listeners.delete(key);
    }
  };
}

export function isWatched(key: string): boolean {
  return (listeners.get(key)?.size ?? 0) > 0;
}

export function publishAudio(key: string, pcm: Float32Array, channels: number): void {
  const set = listeners.get(key);
  if (set === undefined) {
    return;
  }
  for (const listener of set) {
    listener(pcm, channels);
  }
}

export function resetAudioMonitor(): void {
  listeners.clear();
}
