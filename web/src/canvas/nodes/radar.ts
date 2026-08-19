import type { Illuminator, PassiveRadarParams } from "../../lib/types";

/// Where an operator starts from when they say the transmitter's place is known: nowhere in
/// particular, so nothing is drawn on the map until they say where.
export const DEFAULT_ILLUMINATOR: Illuminator = { lat: 0, lon: 0, freq_hz: 100e6 };

export const DEFAULT_RADAR_PARAMS: PassiveRadarParams = {
  cpi_ms: 200,
  max_range_bins: 256,
  doppler_span_hz: 200,
  eca: { delay_taps: 32, doppler_bins: 0, batch_samples: 16_384, loading: 1e-4 },
  cfar: {
    guard_range: 2,
    guard_doppler: 1,
    train_range: 8,
    train_doppler: 4,
    probability_false_alarm: 1e-4,
    min_snr_db: 6,
    zero_doppler_guard: 1,
  },
  illuminator: null,
};

const LIGHT_SPEED_KM_S = 299_792.458;

/// How far out the last range bin reaches, at whatever rate the radio happens to be running.
/// One bin is one sample of extra path, so the answer only needs the rate the surface came at —
/// which the face does not have until a frame arrives, so this is the shape of the axis rather
/// than its length: bins times a microsecond apiece.
export function rangeAxisKm(settings: PassiveRadarParams, rangeStepUs = 1): number {
  return (settings.max_range_bins * rangeStepUs * LIGHT_SPEED_KM_S) / 1e6;
}

/// Half the Doppler span: rows run from minus this to plus this.
export function dopplerAxisHz(settings: PassiveRadarParams): number {
  return settings.doppler_span_hz / 2;
}

/// Which row of the surface a Doppler shift lands on. The middle row is zero, so a target
/// standing still sits in the centre and everything else leans off it.
export function dopplerRow(dopplerHz: number, dopplers: number, stepHz: number): number {
  if (stepHz === 0) {
    return 0;
  }
  return Math.round((dopplers - 1) / 2 + dopplerHz / stepHz);
}

/// The bistatic range a bin stands for, given how long one sample is.
export function rangeKm(bin: number, rangeStepUs: number): number {
  return (bin * rangeStepUs * LIGHT_SPEED_KM_S) / 1e6;
}
