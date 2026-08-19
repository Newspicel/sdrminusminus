import type { QueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  AUDIO_RECORDINGS_KEY,
  BOOKMARKS_KEY,
  CALLS_KEY,
  DECODER_LOG_KEY,
  DEVICES_KEY,
  IMAGES_KEY,
  PRESETS_KEY,
  RECORDINGS_KEY,
  STATE_KEY,
  TEMPLATES_KEY,
  WORKSPACES_KEY,
} from "./api";
import { audioEngine } from "./audio/useChannelAudio";
import { useDecodedStore } from "./decoded";
import { useDfStore } from "./df";
import { useHuntStore } from "./hunt";
import { iqHub } from "./iq";
import { useLevelStore } from "./levels";
import { usePositionStore } from "./position";
import { useScannerStore } from "./scanner";
import { spectrumHub } from "./spectrum";
import { surfaceHub } from "./surface";
import { symbolHub } from "./symbols";
import { pushToast } from "./toasts";
import type {
  CapturedImage,
  CapturedImagesResponse,
  ServerEvent,
  StateScope,
  VoiceCall,
  VoiceCallsResponse,
} from "./types";
import { videoHub } from "./video";
import { SdrSocket } from "./ws";

export function useSdrSocket(queryClient: QueryClient, workspaceError: string | null) {
  const [socket, setSocket] = useState<SdrSocket | null>(null);
  const onServerEvent = useCallback(
    (event: ServerEvent) => {
      switch (event.type) {
        case "Hello":
          void queryClient.invalidateQueries();
          break;
        case "StateChanged":
          invalidateScope(queryClient, event.data.scope);
          break;
        case "Decoded":
          if (event.data.event.kind === "call") {
            appendCall(queryClient, event.data.event.data);
          }
          break;
        case "ImageCaptured":
          appendImage(queryClient, event.data);
          break;
        case "Error":
          if (!audioEngine.claimServerError(event.data.message)) {
            pushToast(event.data.message);
          }
          break;
        default:
          break;
      }
    },
    [queryClient],
  );
  const onServerEventRef = useRef(onServerEvent);
  useLayoutEffect(() => {
    onServerEventRef.current = onServerEvent;
  });

  useEffect(() => {
    const s = new SdrSocket();
    s.on("event", (event) => onServerEventRef.current(event));
    let up = false;
    s.on("status", (now) => {
      if (up && !now) {
        pushToast("Lost the server — reconnecting");
      }
      up = now;
    });
    s.on("event", useDecodedStore.getState().observe);
    s.on("event", useScannerStore.getState().observe);
    s.on("event", useHuntStore.getState().observe);
    s.on("event", usePositionStore.getState().observe);
    s.on("event", useLevelStore.getState().observe);
    s.on("event", useDfStore.getState().observe);
    spectrumHub.attach(s);
    iqHub.attach(s);
    symbolHub.attach(s);
    videoHub.attach(s);
    surfaceHub.attach(s);
    audioEngine.attach(s);
    setSocket(s);
    s.connect();
    return () => {
      spectrumHub.detach();
      iqHub.detach();
      symbolHub.detach();
      videoHub.detach();
      surfaceHub.detach();
      audioEngine.detach();
      s.close();
    };
  }, []);

  useEffect(() => {
    if (workspaceError !== null) {
      pushToast(`Workspace: ${workspaceError}`);
    }
  }, [workspaceError]);

  const retrySocket = useCallback(() => socket?.retryNow(), [socket]);

  return { socket, retrySocket };
}
const MAX_CACHED_CALLS = 10_000;

function appendCall(queryClient: QueryClient, call: VoiceCall) {
  queryClient.setQueryData(CALLS_KEY, (previous: VoiceCallsResponse | undefined) => ({
    calls: [call, ...(previous?.calls ?? [])].slice(0, MAX_CACHED_CALLS),
  }));
}

const MAX_CACHED_IMAGES = 512;

function appendImage(queryClient: QueryClient, image: CapturedImage) {
  queryClient.setQueryData(IMAGES_KEY, (previous: CapturedImagesResponse | undefined) => ({
    images: [image, ...(previous?.images ?? [])].slice(0, MAX_CACHED_IMAGES),
  }));
}

function invalidateScope(queryClient: QueryClient, scope: StateScope): void {
  switch (scope.scope) {
    case "all":
      void queryClient.invalidateQueries();
      break;
    case "devices":
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      void queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
      void queryClient.invalidateQueries({ queryKey: TEMPLATES_KEY });
      break;
    case "device_set":
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      break;
    case "presets":
      void queryClient.invalidateQueries({ queryKey: PRESETS_KEY });
      break;
    case "bookmarks":
      void queryClient.invalidateQueries({ queryKey: BOOKMARKS_KEY });
      break;
    case "recordings":
      void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
      void queryClient.invalidateQueries({ queryKey: AUDIO_RECORDINGS_KEY });
      break;
    case "workspaces":
      void queryClient.invalidateQueries({ queryKey: WORKSPACES_KEY });
      break;
    case "decoder_log":
      void queryClient.invalidateQueries({ queryKey: DECODER_LOG_KEY });
      break;
    case "calls":
      void queryClient.invalidateQueries({ queryKey: CALLS_KEY });
      break;
    case "images":
      void queryClient.invalidateQueries({ queryKey: IMAGES_KEY });
      break;
  }
}
