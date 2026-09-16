import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Upload } from "lucide-react";
import { useRef, useState } from "react";
import { checkUpload, UPLOAD_ACCEPT, UPLOAD_SAID } from "../canvas/nodes/recordingNode";
import { RECORDINGS_KEY, STATE_KEY, uploadRecording } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RecordingInfo } from "../lib/types";
import { Button, Input } from "./BaseControls";
import { BTN, ICON_BTN } from "./controls";
import { Icon } from "./Icon";

const TITLE =
  "Add a SigMF recording from this computer: a .sigmf archive, or a .sigmf-meta and .sigmf-data pair";

export function RecordingUpload({
  compact = false,
  onUploaded,
}: {
  compact?: boolean;
  onUploaded?: (recording: RecordingInfo) => void;
}) {
  const queryClient = useQueryClient();
  const picker = useRef<HTMLInputElement>(null);
  const [said, setSaid] = useState<string | null>(null);

  const report = (message: string): void => {
    if (compact) {
      pushToast(message);
    } else {
      setSaid(message);
    }
  };

  const send = useMutation({
    mutationFn: uploadRecording,
    onSuccess: (recording) => {
      setSaid(null);
      void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      onUploaded?.(recording);
    },
    onError: (error: Error) => report(error.message),
  });

  const offer = (chosen: FileList | null): void => {
    const files = [...(chosen ?? [])];
    const problem = checkUpload(files.map((file) => file.name));
    if (problem === null) {
      send.mutate(files);
    } else {
      report(UPLOAD_SAID[problem]);
    }
  };

  const control = (
    <>
      <Input
        ref={picker}
        type="file"
        aria-label="SigMF files to upload"
        className="hidden"
        multiple
        accept={UPLOAD_ACCEPT}
        onChange={(event) => {
          offer(event.target.files);
          event.target.value = "";
        }}
      />
      <Button
        type="button"
        className={compact ? ICON_BTN : BTN}
        aria-label="Upload SigMF"
        title={TITLE}
        disabled={send.isPending}
        onClick={() => picker.current?.click()}
      >
        <Icon glyph={Upload} />
        {!compact && <span className="ml-1">{send.isPending ? "Uploading…" : "Upload SigMF"}</span>}
      </Button>
    </>
  );

  if (compact) {
    return control;
  }

  return (
    <div className="flex flex-col gap-1">
      {control}
      {said !== null && (
        <p role="alert" className="font-mono text-[10px] text-danger">
          {said}
        </p>
      )}
    </div>
  );
}
