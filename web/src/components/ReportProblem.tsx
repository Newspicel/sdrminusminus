import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { aboutQuery, diagnosticsQuery } from "../lib/api";
import { copyText } from "../lib/copyText";
import { clientEvents, droppedEvents } from "../lib/diagnostics";
import { pushToast } from "../lib/toasts";
import type { PatchGraph } from "../lib/types";
import { Button, Input } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN, BTN_PRIMARY, BTN_QUIET, DIALOG_TITLE, FIELD, LABEL, SURFACE } from "./controls";
import { bugIssueUrl, buildBundle, featureIssueUrl, issueTitle, workspaceFacts } from "./report";

export function ReportProblem({
  open,
  onOpenChange,
  graph,
  seedTitle,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  graph: PatchGraph | null;
  seedTitle?: string | null;
}) {
  const [title, setTitle] = useState("");
  const [includeWorkspace, setIncludeWorkspace] = useState(false);
  const about = useQuery(aboutQuery(open));
  const diagnostics = useQuery(diagnosticsQuery(open));

  const version = about.data?.version ?? diagnostics.data?.doctor.version ?? "";
  const repository = about.data?.repository ?? "";
  const effectiveTitle = issueTitle(title.length > 0 ? title : (seedTitle ?? ""));

  const bundle = useMemo(
    () =>
      buildBundle({
        version,
        userAgent: navigator.userAgent,
        diagnostics: diagnostics.data ?? null,
        events: clientEvents(),
        droppedEvents: droppedEvents(),
        workspace: includeWorkspace && graph !== null ? workspaceFacts(graph) : null,
      }),
    [version, diagnostics.data, includeWorkspace, graph],
  );

  const copy = () => {
    copyText(bundle).then(
      () => pushToast("Diagnostics copied to the clipboard", "info"),
      (error: Error) => pushToast(error.message),
    );
  };

  const openIssue = (href: string) => {
    copyText(bundle).catch(() => undefined);
    window.open(href, "_blank", "noopener,noreferrer");
    onOpenChange(false);
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex max-h-[85vh] w-full max-w-3xl -translate-x-1/2 -translate-y-1/2 flex-col p-4`}
        >
          <div className="flex shrink-0 items-baseline justify-between gap-4">
            <Dialog.Title className={DIALOG_TITLE}>Report a problem</Dialog.Title>
            <Dialog.Description className="legend">
              {version.length > 0 ? `SDR-- ${version}` : "Collecting…"}
            </Dialog.Description>
          </div>

          <div className="mt-3 flex shrink-0 flex-col gap-2">
            <label className="flex items-center gap-2">
              <span className={`${LABEL} w-24`}>Summary</span>
              <Input
                className={`${FIELD} flex-1`}
                name="issue-title"
                value={title}
                placeholder={seedTitle ?? "What went wrong, in one line"}
                onChange={(event) => setTitle(event.target.value)}
              />
            </label>
            <label className="flex items-center gap-2 text-xs text-ink-dim">
              <Checkbox
                label="Include the workspace shape"
                checked={includeWorkspace}
                onChange={setIncludeWorkspace}
              />
              <span title="Node and edge counts per kind. No names, positions or frequencies.">
                Include the workspace shape
              </span>
            </label>
          </div>

          <p className="mt-3 shrink-0 text-xs text-ink-dim">
            This is everything the report will carry. The shared token, your home directory and
            every address but the loopback are already stripped — read it before you publish it.
          </p>

          {diagnostics.isError && (
            <p className="mt-2 shrink-0 text-xs text-danger">
              The server did not answer, so only what this window saw is below.
            </p>
          )}

          <pre className="mt-2 min-h-0 flex-1 overflow-auto rounded-[3px] bg-panel-2 p-2 font-mono text-[11px] whitespace-pre-wrap text-ink-dim">
            {diagnostics.isPending ? "Collecting…" : bundle}
          </pre>

          <div className="mt-3 flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-line pt-3">
            <Button
              type="button"
              className={BTN_QUIET}
              disabled={repository.length === 0}
              onClick={() => openIssue(featureIssueUrl(repository, version))}
            >
              Request a feature instead
            </Button>
            <div className="flex items-center gap-2">
              <Dialog.Close className={BTN}>Close</Dialog.Close>
              <Button type="button" className={BTN} onClick={copy}>
                Copy
              </Button>
              <Button
                type="button"
                className={BTN_PRIMARY}
                disabled={repository.length === 0}
                onClick={() => openIssue(bugIssueUrl(repository, effectiveTitle, version, bundle))}
              >
                Open a GitHub issue
              </Button>
            </div>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
