// Spectrum x-placement for channel markers: the display spans
// [center − span/2, center + span/2] and a channel sits at `offset` from center.
// `null` = out of view (or no usable span yet) → don't render the marker.
export function markerFraction(offsetHz: number, spanHz: number): number | null {
  if (!Number.isFinite(offsetHz) || !(spanHz > 0)) {
    return null;
  }
  const fraction = 0.5 + offsetHz / spanHz;
  return fraction >= 0 && fraction <= 1 ? fraction : null;
}
