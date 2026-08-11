// The band explorer (FEATURES §5): pick the region, search it in words or in megahertz, and
// tune what you find with the mode the band suggests.
//
// It lives in the library drawer beside presets and bookmarks because it is the same kind of
// thing — something that configures the radios the patch names, rather than a node with a stream
// to carry. It is also the keyboard route into the band plan: the ruler on the scope is a
// pointer instrument, this is a list.
import { useState } from "react";
import { locateBandRegion } from "../lib/api";
import { setBandRegion } from "../lib/bandRegion";
import { pushToast } from "../lib/toasts";
import type { ChannelParams, DeviceSet } from "../lib/types";
import { useBandPlan } from "../lib/useBandPlan";
import { useDevicePatch } from "../lib/useDevicePatch";
import { searchPlan, serviceEdge, serviceLabel } from "./bandPlan";
import { BTN, CHIP, FIELD, LABEL } from "./controls";
import { formatHz } from "./format";

/** Enough to fill the drawer without turning a two-letter query into a wall. */
const LIMIT = 30;

export function BandsPanel({ active }: { active: DeviceSet | null }) {
  const { region, regions, plan } = useBandPlan();
  const { applyPatch } = useDevicePatch();
  const [query, setQuery] = useState("");
  const [locating, setLocating] = useState(false);

  const hits = plan === null ? [] : searchPlan(plan, query, LIMIT);

  /** Tune the receiver to the band's centre. Only the receiver: this panel has no notion of
   * which channel is selected — the scope's ruler is where a band moves a channel — and it is
   * the same thing the bookmarks beside it do. */
  const tune = (startHz: number, stopHz: number, suggested: ChannelParams | null): void => {
    if (active === null) {
      return;
    }
    applyPatch(active.id, { center_hz: startHz + (stopHz - startHz) / 2 });
    if (suggested !== null) {
      pushToast(`${suggested.type.toUpperCase()} is the mode for this band — set it on a channel`);
    }
  };

  /** Best-effort only: `navigator.geolocation` needs a secure context, and this server is
   * ordinarily plain HTTP on a LAN, so the button is expected to fail in exactly the deployed
   * case. Choosing the region by hand is the primary path; this is a shortcut, and it says so
   * when it cannot work. */
  const detect = (): void => {
    if (!("geolocation" in navigator)) {
      pushToast("This browser offers no location; choose a region instead");
      return;
    }
    setLocating(true);
    navigator.geolocation.getCurrentPosition(
      (position) => {
        void locateBandRegion(position.coords.latitude, position.coords.longitude)
          .then((found) => {
            setBandRegion(found.region);
            if (found.approximate) {
              pushToast(
                `Only the ITU region could be decided from here — check the region is right`,
              );
            }
          })
          .catch((error: Error) => pushToast(error.message))
          .finally(() => setLocating(false));
      },
      (error) => {
        setLocating(false);
        // The usual cause is not a refusal but an insecure origin, which the browser also
        // reports as "permission denied"; say both rather than blame the operator.
        pushToast(`No location: ${error.message} (needs HTTPS or localhost)`);
      },
      { maximumAge: 600_000, timeout: 10_000 },
    );
  };

  return (
    <div className="flex flex-col gap-2 p-3">
      <div className="flex items-center gap-2">
        <select
          className={`${FIELD} min-w-0 flex-1`}
          aria-label="Band plan region"
          value={region ?? ""}
          onChange={(event) => setBandRegion(event.target.value)}
        >
          {regions.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </select>
        <button type="button" className={BTN} onClick={detect} disabled={locating}>
          {locating ? "Locating…" : "Detect"}
        </button>
      </div>

      <input
        className={FIELD}
        placeholder="marine VHF, 70 cm ham, 145.500…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        aria-label="Search the band plan"
      />

      {plan === null && <span className="text-sm text-ink-dim">Loading the band plan…</span>}
      {plan !== null && query.trim() === "" && (
        <span className="text-sm text-ink-dim">
          Search {plan.region.name} by service, band name, wavelength or frequency.
        </span>
      )}
      {plan !== null && query.trim() !== "" && hits.length === 0 && (
        <span className="text-sm text-ink-dim">Nothing in {plan.region.name} matches that.</span>
      )}

      {hits.map((hit) => {
        const { allocation } = hit;
        return (
          <div key={`${hit.laneId}:${allocation.id}`} className="flex items-start gap-2">
            <button
              type="button"
              className="min-w-0 flex-1 rounded px-1 py-1 text-left transition-colors hover:bg-panel-2 disabled:opacity-40"
              disabled={active === null}
              onClick={() =>
                tune(allocation.start_hz, allocation.stop_hz, allocation.suggested ?? null)
              }
            >
              <span className="flex items-center gap-1.5">
                <span
                  aria-hidden
                  className={`size-2 shrink-0 rounded-[1px] ${serviceEdge(allocation.service)}`}
                />
                <span className="min-w-0 truncate text-sm text-ink">{allocation.name}</span>
                {allocation.suggested != null && (
                  <span className={CHIP}>{allocation.suggested.type}</span>
                )}
              </span>
              <span className={`${LABEL} mt-0.5`}>
                {formatHz(allocation.start_hz)}–{formatHz(allocation.stop_hz)} ·{" "}
                {serviceLabel(allocation.service)}
                {hit.laneId !== "allocation" && ` · ${hit.laneName}`}
              </span>
              {allocation.notes != null && (
                <p className="mt-0.5 text-xs leading-snug text-ink-dim">{allocation.notes}</p>
              )}
            </button>
          </div>
        );
      })}
    </div>
  );
}
