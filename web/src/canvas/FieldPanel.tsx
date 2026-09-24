import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { renderSVG } from "uqr";
import { Button } from "../components/BaseControls";
import { segmentSm, WELL } from "../components/controls";
import { aboutQuery } from "../lib/api";
import { handoffOrigins, handoffUrl } from "./fieldLink";

/// Hands the field client to a phone.
///
/// A QR code rather than a typed address, because the token in it is long and the operator is
/// standing next to a car.
export function FieldPanel() {
  const about = useQuery(aboutQuery(true));
  const origins = useMemo(
    () => handoffOrigins(window.location.origin, about.data?.lan_addresses ?? []),
    [about.data?.lan_addresses],
  );
  const [pick, setPick] = useState(0);
  const origin = origins[Math.min(pick, origins.length - 1)] ?? window.location.origin;
  const url = handoffUrl(origin);
  if (about.data?.local_only) {
    return (
      <p className="mx-auto w-72 max-w-full p-3 text-center text-xs text-ink-faint">
        The server only listens on this machine. Start it with <code>--bind 0.0.0.0:8080</code>.
      </p>
    );
  }
  return (
    <div className="mx-auto flex w-72 max-w-full flex-col items-center gap-2 p-3">
      {/* biome-ignore lint/security/noDangerouslySetInnerHtml: uqr renders an SVG string, no user input */}
      <div
        hidden
        className="rounded bg-white p-2"
        role="img"
        aria-label="Field mode QR code"
        // biome-ignore lint/security/noDangerouslySetInnerHtml: as above
        dangerouslySetInnerHTML={{ __html: renderSVG(url, { border: 1 }) }}
      />
      <code className="legend w-full break-all text-center">{url}</code>
      {origins.length > 1 && (
        <div className={`${WELL} flex-wrap justify-center`}>
          {origins.map((candidate, index) => (
            <Button
              key={candidate}
              type="button"
              aria-pressed={index === pick}
              className={segmentSm(index === pick)}
              onClick={() => setPick(index)}
            >
              {new URL(candidate).host}
            </Button>
          ))}
        </div>
      )}
      {origins.length === 1 && (about.data?.lan_addresses?.length ?? 0) === 0 && (
        <p className="text-center text-xs text-ink-faint">
          This machine reports no network address a phone could reach it at.
        </p>
      )}
    </div>
  );
}
