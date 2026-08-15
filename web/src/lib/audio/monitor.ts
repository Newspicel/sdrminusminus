// A tap on the decoded audio, for displays that want to look at what is being heard.
//
// The audio path already decodes Opus to PCM for playback; an audio spectrogram is a second
// reader of exactly those samples, not a second stream. So this is a registry, not a pipeline: a
// sink publishes each decoded block under its channel's key, and nothing is computed unless
// something is watching.
//
// It follows that the tap only carries audio while the channel is *playing*. That is the honest
// behaviour for a monitor of what you are hearing, and it is why nothing here tries to subscribe
// on a watcher's behalf.

/** One decoded block: interleaved PCM at 48 kHz, and how many channels it is interleaved at. */
export type PcmListener = (pcm: Float32Array, channels: number) => void;

const listeners = new Map<string, Set<PcmListener>>();

export function monitorKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

/** Watch one channel's decoded audio. Returns the unsubscribe. */
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

/** Whether anything is watching. The sink checks this before publishing, so a channel nobody is
 * looking at costs one map lookup per decoded block and nothing else. */
export function isWatched(key: string): boolean {
  return (listeners.get(key)?.size ?? 0) > 0;
}

/**
 * Hand a decoded block to whoever is watching.
 *
 * `pcm` belongs to the decoder and is reused, so a listener that needs to keep it must copy —
 * which is stated here rather than defended against, because the only listener is a transform
 * that consumes it immediately and a copy per block would be pure waste.
 */
export function publishAudio(key: string, pcm: Float32Array, channels: number): void {
  const set = listeners.get(key);
  if (set === undefined) {
    return;
  }
  for (const listener of set) {
    listener(pcm, channels);
  }
}

/** Drop every watcher. The test seam; nothing in the app calls it. */
export function resetAudioMonitor(): void {
  listeners.clear();
}
