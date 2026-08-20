import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { renderSVG } from "uqr";
import { Button } from "../components/BaseControls";
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
  return (
    <div className="mx-auto flex w-72 max-w-full flex-col items-center gap-2 p-3">
      {/* biome-ignore lint/security/noDangerouslySetInnerHtml: uqr renders an SVG string, no user input */}
      <div
        className="rounded bg-white p-2"
        role="img"
        aria-label="Field mode QR code"
        // biome-ignore lint/security/noDangerouslySetInnerHtml: as above
        dangerouslySetInnerHTML={{ __html: renderSVG(url, { border: 1 }) }}
      />
      <code className="w-full break-all text-center text-[10px] text-ink-dim">{url}</code>
      {origins.length > 1 && (
        <div className="flex flex-wrap justify-center gap-1">
          {origins.map((candidate, index) => (
            <Button
              key={candidate}
              type="button"
              className={`rounded px-2 py-1 text-[10px] ${index === pick ? "bg-accent text-bg" : "border border-line"}`}
              onClick={() => setPick(index)}
            >
              {new URL(candidate).host}
            </Button>
          ))}
        </div>
      )}
      {origins.length === 1 && (about.data?.lan_addresses?.length ?? 0) === 0 && (
        <p className="text-center text-[10px] text-ink-dim">
          This machine reports no network address a phone could reach it at.
        </p>
      )}
    </div>
  );
}
