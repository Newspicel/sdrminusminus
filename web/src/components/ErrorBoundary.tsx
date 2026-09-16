import { Component, type ErrorInfo, type ReactNode } from "react";
import { describeError, recordEvent } from "../lib/diagnostics";
import { Button } from "./BaseControls";
import { BTN, BTN_PRIMARY } from "./controls";
import { ReportProblem } from "./ReportProblem";

interface State {
  error: Error | null;
  reporting: boolean;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, reporting: false };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    recordEvent("error", "render", describeError(error));
    if (info.componentStack) {
      recordEvent("error", "render", info.componentStack);
    }
  }

  render() {
    const { error, reporting } = this.state;
    if (error === null) {
      return this.props.children;
    }
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center bg-bg px-4 py-10">
        <div
          role="alert"
          className="flex w-full max-w-md flex-col gap-3 rounded border border-line bg-panel px-4 py-4"
        >
          <div className="flex items-center gap-2">
            <img src="/icon.svg" alt="" width={28} height={28} className="shrink-0" />
            <div className="font-mono text-lg font-semibold text-accent">SDR--</div>
          </div>
          <div>
            <h1 className="text-sm font-semibold text-ink">This window stopped drawing</h1>
            <p className="mt-1 text-sm text-ink-dim">
              The server kept running and nothing you arranged is lost — it lives there, not here.
              Reloading picks it up again.
            </p>
          </div>
          <p className="font-mono text-xs break-words text-ink-faint">{error.message}</p>
          <div className="flex items-center gap-2">
            <Button type="button" className={BTN_PRIMARY} onClick={() => window.location.reload()}>
              Reload
            </Button>
            <Button
              type="button"
              className={BTN}
              onClick={() => this.setState({ reporting: true })}
            >
              Report this
            </Button>
          </div>
          <ReportProblem
            open={reporting}
            onOpenChange={(open) => this.setState({ reporting: open })}
            graph={null}
            seedTitle={error.message}
          />
        </div>
      </div>
    );
  }
}
