import { useCallback, useEffect, useSyncExternalStore } from "react";
import type { SdrSocket } from "../ws";
import { AudioEngine } from "./engine";
import {
  createWebAudioSink,
  onOutputRerouted,
  onOutputStateChange,
  resumeAudioOutput,
} from "./sink";

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

const NO_FX: readonly string[] = [];

export const audioEngine = new AudioEngine(createWebAudioSink);
onOutputStateChange((running) => audioEngine.setOutputRunning(running));
onOutputRerouted(() => audioEngine.rerouteOutput());

export function useChannelAudio(
  socket: SdrSocket | null,
  deviceSet: number,
  channelId: number,
  fx: readonly string[] = NO_FX,
): ChannelAudio {
  useEffect(() => {
    if (socket) {
      audioEngine.attach(socket);
    }
  }, [socket]);

  const playing = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.isPlaying(deviceSet, channelId, fx),
  );
  const pending = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.isPending(deviceSet, channelId, fx),
  );
  const suspended = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.isSuspended(deviceSet, channelId, fx),
  );
  const error = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getError(deviceSet, channelId, fx),
  );
  const volume = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getVolume(deviceSet, channelId, fx),
  );
  const lostFrames = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getLostFrames(deviceSet, channelId, fx),
  );
  const underruns = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getUnderruns(deviceSet, channelId, fx),
  );

  const bufferedMs = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getBufferedMs(deviceSet, channelId, fx),
  );
  const trimmedMs = useSyncExternalStore(audioEngine.subscribe, () =>
    audioEngine.getTrimmedMs(deviceSet, channelId, fx),
  );

  const start = useCallback(() => {
    if (!socket) {
      return;
    }
    audioEngine.attach(socket);
    audioEngine.start(deviceSet, channelId, fx);
  }, [socket, deviceSet, channelId, fx]);

  const stop = useCallback(() => {
    audioEngine.stop(deviceSet, channelId, fx);
  }, [deviceSet, channelId, fx]);

  const dismissError = useCallback(() => {
    audioEngine.clearError(deviceSet, channelId, fx);
  }, [deviceSet, channelId, fx]);

  const setVolume = useCallback(
    (v: number) => {
      audioEngine.setVolume(deviceSet, channelId, v, fx);
    },
    [deviceSet, channelId, fx],
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
