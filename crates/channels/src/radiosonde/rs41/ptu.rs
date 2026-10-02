pub(crate) const SUBFRAMES: usize = 51;
pub(crate) const SUBFRAME_LEN: usize = 16;
pub(crate) const CALIBRATION_LEN: usize = SUBFRAMES * SUBFRAME_LEN;

pub(crate) const REF_RESISTOR_LOW: usize = 61;
pub(crate) const REF_RESISTOR_HIGH: usize = 65;
pub(crate) const TEMPERATURE_POLY: usize = 77;
pub(crate) const TEMPERATURE_CAL: usize = 89;
pub(crate) const HUMIDITY_CAL: usize = 117;
pub(crate) const SONDE_TYPE: usize = 0x218;
pub(crate) const PRESSURE_FLAG: usize = 0x21F;
pub(crate) const PRESSURE_SCALE_INDEX: usize = 24;
pub(crate) const PRESSURE_COEFFICIENTS: [(usize, usize); 18] = [
    (606, 0),
    (610, 4),
    (614, 8),
    (618, 12),
    (622, 16),
    (626, 20),
    (630, 24),
    (634, 1),
    (638, 5),
    (642, 9),
    (646, 13),
    (650, 2),
    (654, 6),
    (658, 10),
    (662, 14),
    (666, 3),
    (670, 7),
    (674, 11),
];

const TEMPERATURE_SUBFRAMES: [usize; 4] = [3, 4, 5, 6];
const HUMIDITY_SUBFRAMES: [usize; 1] = [7];
const PRESSURE_SUBFRAMES: [usize; 7] = [0x21, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A];
const HUMIDITY_OFFSET: f64 = 7.5;
const HUMIDITY_GAIN: f64 = 350.0;
const PLAUSIBLE_CELSIUS: std::ops::RangeInclusive<f64> = -120.0..=80.0;
const PLAUSIBLE_HPA: std::ops::RangeInclusive<f64> = 0.0..=1_200.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Measurement {
    pub signal: f64,
    pub low: f64,
    pub high: f64,
}

impl Measurement {
    pub(crate) fn ratio(self) -> Option<f64> {
        let span = self.high - self.low;
        (span != 0.0).then(|| (self.signal - self.low) / span)
    }
}

pub(crate) struct Calibration {
    bytes: [u8; CALIBRATION_LEN],
    have: [bool; SUBFRAMES],
    serial: [u8; 8],
}

impl Calibration {
    pub(crate) fn new() -> Self {
        Self {
            bytes: [0; CALIBRATION_LEN],
            have: [false; SUBFRAMES],
            serial: [0; 8],
        }
    }

    pub(crate) fn store(&mut self, serial: [u8; 8], index: u8, data: &[u8]) {
        if serial != self.serial {
            self.bytes = [0; CALIBRATION_LEN];
            self.have = [false; SUBFRAMES];
            self.serial = serial;
        }
        let index = usize::from(index);
        if index >= SUBFRAMES || data.len() != SUBFRAME_LEN {
            return;
        }
        let start = index * SUBFRAME_LEN;
        self.bytes[start..start + SUBFRAME_LEN].copy_from_slice(data);
        self.have[index] = true;
    }

    fn has(&self, subframes: &[usize]) -> bool {
        subframes.iter().all(|&index| self.have[index])
    }

    fn float(&self, offset: usize) -> f64 {
        let mut word = [0u8; 4];
        word.copy_from_slice(&self.bytes[offset..offset + 4]);
        f64::from(f32::from_le_bytes(word))
    }

    fn floats<const N: usize>(&self, offset: usize) -> [f64; N] {
        std::array::from_fn(|k| self.float(offset + 4 * k))
    }

    pub(crate) fn temperature(&self, m: Measurement) -> Option<f64> {
        if !self.has(&TEMPERATURE_SUBFRAMES) {
            return None;
        }
        let (r_low, r_high) = (self.float(REF_RESISTOR_LOW), self.float(REF_RESISTOR_HIGH));
        let poly: [f64; 3] = self.floats(TEMPERATURE_POLY);
        let cal: [f64; 3] = self.floats(TEMPERATURE_CAL);
        let gain = (m.high - m.low) / (r_high - r_low);
        let bias = (m.low * r_high - m.high * r_low) / (m.high - m.low);
        let resistance = (m.signal / gain - bias) * cal[0];
        let celsius = (poly[0] + poly[1] * resistance + poly[2] * resistance * resistance + cal[1])
            * (1.0 + cal[2]);
        PLAUSIBLE_CELSIUS.contains(&celsius).then_some(celsius)
    }

    pub(crate) fn humidity(&self, m: Measurement, celsius: f64) -> Option<f64> {
        if !self.has(&HUMIDITY_SUBFRAMES) {
            return None;
        }
        let scale = HUMIDITY_GAIN / self.float(HUMIDITY_CAL);
        let raw = 100.0 * (scale * m.ratio()? - HUMIDITY_OFFSET) - celsius / 5.5;
        let compensated = humidity_cold_compensation(celsius) * raw;
        compensated
            .is_finite()
            .then(|| compensated.clamp(0.0, 100.0))
    }

    pub(crate) fn pressure(&self, m: Measurement, sensor_celsius: f64) -> Option<f64> {
        if !self.has(&PRESSURE_SUBFRAMES) || self.bytes[PRESSURE_FLAG] != b'P' {
            return None;
        }
        let mut coefficients = [0.0f64; 25];
        for (offset, index) in PRESSURE_COEFFICIENTS {
            coefficients[index] = self.float(offset);
        }
        let ratio = m.ratio()?;
        if ratio == 0.0 {
            return None;
        }
        let a0 = coefficients[PRESSURE_SCALE_INDEX] / ratio;
        let mut hpa = 0.0;
        let mut a0_power = 1.0;
        for row in coefficients.as_chunks::<4>().0.iter().take(6) {
            let mut a1_power = 1.0;
            for &coefficient in row {
                hpa += a0_power * a1_power * coefficient;
                a1_power *= sensor_celsius;
            }
            a0_power *= a0;
        }
        PLAUSIBLE_HPA.contains(&hpa).then_some(hpa)
    }
}

pub(crate) fn humidity_cold_compensation(celsius: f64) -> f64 {
    let mut factor = 1.0;
    if celsius < -20.0 {
        factor *= 1.0 + (-20.0 - celsius) / 100.0;
    }
    if celsius < -40.0 {
        factor *= 1.0 + (-40.0 - celsius) / 120.0;
    }
    factor
}

pub(crate) fn humidity_ratio(humidity_cal: f64, humidity_pct: f64, celsius: f64) -> f64 {
    let raw = humidity_pct / humidity_cold_compensation(celsius) + celsius / 5.5;
    (raw / 100.0 + HUMIDITY_OFFSET) * humidity_cal / HUMIDITY_GAIN
}
