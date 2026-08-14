// Per-channel audio playback hook — the contract between the audio subsystem (lib/audio/)
// and the channel UI. State lives in a module-level engine so playback survives panel
// remounts and multiple panels for the same channel stay in sync.
import { useCallback, useEffect, useSyncExternalStore } from "react";
import type { SdrSocket } from "../ws";
import { AudioEngine } from "./engine";
import { createWebAudioSink, onOutputStateChange, resumeAudioOutput } from "./sink";

export interface ChannelAudio {
  playing: boolean;
  /** Start requested but no stream bound yet (subscribe in flight or socket down). */
  pending: boolean;
  /** Stream bound but the platform suspended the output (autoplay veto, phone call). */
  suspended: boolean;
  /** Why playback stopped without the user asking; null when none. */
  error: string | null;
  start: () => void;
  stop: () => void;
  dismissError: () => void;
  /** Wire to a click/tap handler: iOS only resumes audio inside a user gesture. */
  resumeOutput: () => void;
  /** Client-side gain, 0..1. */
  volume: number;
  setVolume: (v: number) => void;
  /**
   * Why the sound broke up, split by who is responsible. `lostFrames` is audio that never
   * reached the browser — dropped at the radio, the encoder or the socket. `underruns` is audio
   * that did arrive and still could not be played in time, which is this machine's scheduling.
   * One is a reason to look at the server or the link; the other is not.
   */
  lostFrames: number;
  underruns: number;
}

/** Exported so the app shell can route `ServerEvent::Error` through `claimServerError`. */
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
  };
}
