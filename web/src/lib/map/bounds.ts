export type MapBounds = [[number, number], [number, number]];

export function unwrapTrail(coordinates: readonly [number, number][]): [number, number][] {
  const unwrapped: [number, number][] = [];
  let previousLongitude = coordinates[0]?.[0] ?? 0;
  for (const [rawLongitude, latitude] of coordinates) {
    const longitude = rawLongitude + 360 * Math.round((previousLongitude - rawLongitude) / 360);
    unwrapped.push([longitude, latitude]);
    previousLongitude = longitude;
  }
  return unwrapped;
}

export function trailBounds(coordinates: readonly [number, number][]): MapBounds | null {
  const unwrapped = unwrapTrail(coordinates);
  if (unwrapped.length === 0) {
    return null;
  }
  const longitudes = unwrapped.map(([longitude]) => longitude);
  const latitudes = unwrapped.map(([, latitude]) => latitude);
  return [
    [Math.min(...longitudes), Math.min(...latitudes)],
    [Math.max(...longitudes), Math.max(...latitudes)],
  ];
}
