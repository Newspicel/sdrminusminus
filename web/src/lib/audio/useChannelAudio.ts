import { useCallback, useEffect, useSyncExternalStore } from "react";
import type { SdrSocket } from "../ws";
import { AudioEngine } from "./engine";
import { createWebAudioSink, onOutputStateChange, resumeAudioOutput } from "./sink";

export interface ChannelAudio {
  playing: boolean;
  pending: boolean;
  suspended: boolean;
  error: string | null;
  start: () => void;
  stop: () => void;
  dismissError: () => void;
  resumeOutput: () => void;
  volume: number;
  setVolume: (v: number) => void;
  lostFrames: number;
  underruns: number;
  bufferedMs?: number;
  trimmedMs?: number;
}

export const audioEngine = new AudioEngine(createWebAudioSink);
onOutputStateChange((running) => audioEngine.setOutputRunning(running));

export function useChannelAudio(
  socket: SdrSocket | null,
  deviceSet: number,
  channelId: number,
): ChannelAudio {
  useEffect(() => {
    if (socket) {
      audioEngine.attach(socket);
    }
  }, [socket]);

  const playing = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.isPlaying(deviceSet, channelId),
  );
  const pending = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.isPending(deviceSet, channelId),
  );
  const suspended = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.isSuspended(deviceSet, channelId),
  );
  const error = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getError(deviceSet, channelId),
  );
  const volume = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getVolume(deviceSet, channelId),
  );
  const lostFrames = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getLostFrames(deviceSet, channelId),
  );
  const underruns = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getUnderruns(deviceSet, channelId),
  );

  const bufferedMs = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getBufferedMs(deviceSet, channelId),
  );
  const trimmedMs = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getTrimmedMs(deviceSet, channelId),
  );

  const start = useCallback(() => {
    if (!socket) {
      return;
    }
    audioEngine.attach(socket);
    audioEngine.start(deviceSet, channelId);
  }, [socket, deviceSet, channelId]);

  const stop = useCallback(() => {
    audioEngine.stop(deviceSet, channelId);
  }, [deviceSet, channelId]);

  const dismissError = useCallback(() => {
    audioEngine.clearError(deviceSet, channelId);
  }, [deviceSet, channelId]);

  const setVolume = useCallback(
    (v: number) => {
      audioEngine.setVolume(deviceSet, channelId, v);
    },
    [deviceSet, channelId],
  );

  return {
    playing,
    pending,
    suspended,
    error,
    start,
    stop,
    dismissError,
    resumeOutput: resumeAudioOutput,
    volume,
    setVolume,
    lostFrames,
    underruns,
    bufferedMs,
    trimmedMs,
  };
}
