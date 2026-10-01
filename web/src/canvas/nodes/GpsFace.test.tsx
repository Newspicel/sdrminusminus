import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReactFlowProvider } from "@xyflow/react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { PHONES_KEY } from "../../lib/api";
import { usePositionStore } from "../../lib/position";
import type { PatchNode, PhonesResponse, PositionFix } from "../../lib/types";
import { stubWorkspace } from "../../test/faceHarness";
import { placed } from "../../test/fixtures";
import { WorkspaceProvider } from "../context";
import { GpsFace } from "./GpsFace";

const PHONE = "p0123456789abcdef";

const NODE: PatchNode = placed("gps1", {
  kind: "gps",
  data: { source: { type: "phone", phone: PHONE } },
});

function phones(online: boolean, listed = true): PhonesResponse {
  return {
    phones: listed
      ? [
          {
            id: PHONE,
            name: "Field phone",
            platform: "android",
            created_at: "2026-09-01T10:00:00Z",
            online,
          },
        ]
      : [],
    access: {
      access: { enabled: false, port: 8443 },
      listener: { state: "off" },
      mdns: { state: "off" },
      protocol: 1,
    },
  };
}

function render(response: PhonesResponse, fix: PositionFix | null, error: string | null): string {
  usePositionStore.setState({ sources: { gps1: { fix, error, history: [] } } });
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  queryClient.setQueryData(PHONES_KEY, response);
  return renderToStaticMarkup(
    <QueryClientProvider client={queryClient}>
      <ReactFlowProvider>
        <WorkspaceProvider value={stubWorkspace({ graph: { nodes: [NODE], edges: [] } })}>
          <GpsFace node={NODE} />
        </WorkspaceProvider>
      </ReactFlowProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => usePositionStore.getState().clear());

describe("GpsFace with a phone", () => {
  it("names the phone, says it is online and reads its heading and tilt", () => {
    const html = render(
      phones(true),
      {
        latitude: 52.52,
        longitude: 13.405,
        time: new Date().toISOString(),
        heading_deg: 123,
        heading_accuracy_deg: 5,
        heading_source: "compass",
        pitch_deg: 2,
        roll_deg: -1,
      },
      null,
    );
    expect(html).toContain("Field phone");
    expect(html).toContain(">Online<");
    expect(html).toContain("52.520000, 13.405000");
    expect(html).toContain(">123° compass ±5°<");
    expect(html).toContain(">2° / -1°<");
    expect(html).toMatch(/>Fix<\/dt><dd[^>]*>0 </);
  });

  it("shows offline and not paired in the header, not as a fault", () => {
    const offline = render(phones(true), null, "phone offline");
    expect(offline).toContain(">Offline<");
    expect(offline).not.toContain('role="alert"');
    const gone = render(phones(false, false), null, "phone offline");
    expect(gone).toContain(">Not paired<");
    expect(gone).toContain(PHONE);
    const silent = render(phones(true), null, "phone silent");
    expect(silent).toContain(">Online<");
    expect(silent).toContain(">phone silent<");
  });
});
