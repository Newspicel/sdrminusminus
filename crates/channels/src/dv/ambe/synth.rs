use std::{
    f32::consts::TAU,
    f64::consts::{E, PI},
};

use super::params::{MAX_HARMONICS, Params};

pub(crate) const FRAME_SAMPLES: usize = 160;
const RAMP_START: usize = 55;
const RAMP_LENGTH: usize = 50;
const NOISE_TONES: usize = 3;
const NOISE_STEP: f32 = 1.0 / NOISE_TONES as f32;
const NOISE_SPREAD: f32 = (NOISE_STEP * (NOISE_TONES - 1) as f32) / 2.0;
const NOISE_EXCESS_GAIN: f32 = 2.0;
const NOISE_THRESHOLD: f32 = ((2_700.0 * PI) / 4_000.0) as f32;
const NOISE_SINE_GAIN: f32 = (1.359_140_9_f32 as f64 * E) as f32;

const fn window(n: usize) -> f32 {
    let edge = if n < FRAME_SAMPLES {
        n
    } else {
        2 * FRAME_SAMPLES - n
    };
    if edge <= RAMP_START {
        0.0
    } else if edge >= RAMP_START + RAMP_LENGTH {
        1.0
    } else {
        (edge - RAMP_START) as f32 / RAMP_LENGTH as f32
    }
}

const FADE_IN: [f32; FRAME_SAMPLES] = {
    let mut ramp = [0.0; FRAME_SAMPLES];
    let mut n = 0;
    while n < FRAME_SAMPLES {
        ramp[n] = window(n);
        n += 1;
    }
    ramp
};

const FADE_OUT: [f32; FRAME_SAMPLES] = {
    let mut ramp = [0.0; FRAME_SAMPLES];
    let mut n = 0;
    while n < FRAME_SAMPLES {
        ramp[n] = window(n + FRAME_SAMPLES);
        n += 1;
    }
    ramp
};

pub(super) struct Noise(u32);

impl Noise {
    const SEED: u32 = 0x9E37_79B9;

    pub(super) const fn new() -> Self {
        Self(Self::SEED)
    }

    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    fn uniform(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    fn phase(&mut self) -> f32 {
        self.uniform() * (std::f32::consts::PI * 2.0) - std::f32::consts::PI
    }
}

pub(super) fn enhance(params: &mut Params) {
    let harmonics = params.harmonics;
    let w0 = params.w0;
    let (rm0, rm1) = (1..=harmonics).fold((0.0f32, 0.0f32), |(rm0, rm1), l| {
        let power = params.magnitude[l] * params.magnitude[l];
        (rm0 + power, rm1 + power * (w0 * l as f32).cos())
    });
    let (r2m0, r2m1) = (rm0 * rm0, rm1 * rm1);

    for l in 1..=harmonics {
        let magnitude = params.magnitude[l];
        if magnitude == 0.0 || 8 * l <= harmonics {
            continue;
        }
        let tilt = (r2m0 + r2m1) - (2.0 * rm0 * rm1 * (w0 * l as f32).cos());
        let base = (0.96f32 as f64 * PI * f64::from(tilt)) / f64::from(w0 * rm0 * (r2m0 - r2m1));
        let weight = magnitude.sqrt() * (base as f32).powf(0.25);
        params.magnitude[l] = if f64::from(weight) > 1.2 {
            (1.2 * f64::from(magnitude)) as f32
        } else if weight < 0.5 {
            0.5 * magnitude
        } else {
            weight * magnitude
        };
    }

    let energy = params.magnitude[1..=harmonics]
        .iter()
        .fold(0.0, |sum, m| sum + m * m);
    let gain = if energy == 0.0 {
        1.0
    } else {
        (rm0 / energy).sqrt()
    };
    for magnitude in &mut params.magnitude[1..=harmonics] {
        *magnitude *= gain;
    }
}

pub(super) fn synthesize(
    pcm: &mut [f32; FRAME_SAMPLES],
    current: &mut Params,
    previous: &mut Params,
    noise: &mut Noise,
) {
    let unvoiced = (1..=current.harmonics)
        .filter(|&l| !current.voiced[l])
        .count();
    let bands = align_band_counts(current, previous);
    advance_phases(current, previous, unvoiced, noise);
    let noise_gain = ((NOISE_TONES as f64).ln() / NOISE_TONES as f64) as f32;

    pcm.fill(0.0);
    for l in 1..=bands {
        let fading_out = Band::new(previous, l, &FADE_OUT, 0, noise);
        let fading_in = Band::new(current, l, &FADE_IN, FRAME_SAMPLES, noise);
        let (first, second) = if fading_in.is_voiced() && !fading_out.is_voiced() {
            (&fading_in, &fading_out)
        } else {
            (&fading_out, &fading_in)
        };
        for (n, sample) in pcm.iter_mut().enumerate() {
            let a = first.sample(n, noise_gain, noise);
            let b = second.sample(n, noise_gain, noise);
            *sample = *sample + a + b;
        }
    }
}

fn align_band_counts(current: &mut Params, previous: &mut Params) -> usize {
    if current.harmonics > previous.harmonics {
        previous.clear_bands(previous.harmonics + 1..=current.harmonics);
        current.harmonics
    } else {
        current.clear_bands(current.harmonics + 1..=previous.harmonics);
        previous.harmonics
    }
}

fn advance_phases(current: &mut Params, previous: &Params, unvoiced: usize, noise: &mut Noise) {
    let step = previous.w0 + current.w0;
    let coherent = current.harmonics / 4;
    for l in 1..=MAX_HARMONICS {
        let track =
            (previous.phase_track[l] + step * ((l * FRAME_SAMPLES) as f32 / 2.0)).rem_euclid(TAU);
        current.phase_track[l] = track;
        current.phase[l] = if l <= coherent {
            track
        } else {
            track + (unvoiced as f32 * noise.phase()) / current.harmonics as f32
        };
    }
}

enum Source {
    Voiced {
        w0l: f32,
        phase: f32,
    },
    Unvoiced {
        w0: f32,
        excess: Option<f32>,
        tones: [(f32, f32); NOISE_TONES],
    },
}

struct Band<'a> {
    source: Source,
    magnitude: f32,
    window: &'a [f32; FRAME_SAMPLES],
    delay: usize,
}

impl<'a> Band<'a> {
    fn new(
        params: &Params,
        l: usize,
        window: &'a [f32; FRAME_SAMPLES],
        delay: usize,
        noise: &mut Noise,
    ) -> Self {
        let w0l = params.w0 * l as f32;
        let source = if params.voiced[l] {
            Source::Voiced {
                w0l,
                phase: params.phase[l],
            }
        } else {
            Source::Unvoiced {
                w0: params.w0,
                excess: (w0l > NOISE_THRESHOLD).then_some(w0l - NOISE_THRESHOLD),
                tones: std::array::from_fn(|i| {
                    let multiple = l as f32 + (i as f32 * NOISE_STEP) - NOISE_SPREAD;
                    (multiple, noise.phase())
                }),
            }
        };
        Self {
            source,
            magnitude: params.magnitude[l],
            window,
            delay,
        }
    }

    const fn is_voiced(&self) -> bool {
        matches!(self.source, Source::Voiced { .. })
    }

    fn sample(&self, n: usize, noise_gain: f32, noise: &mut Noise) -> f32 {
        match self.source {
            Source::Voiced { w0l, phase } => {
                let time = n as f32 - self.delay as f32;
                self.window[n] * self.magnitude * (w0l * time + phase).cos()
            }
            Source::Unvoiced { w0, excess, tones } => {
                let t = w0 * n as f32;
                let mix = tones.iter().fold(0.0, |sum, &(multiple, phase)| {
                    let sum = sum + (t * multiple + phase).cos();
                    excess.map_or(sum, |excess| {
                        sum + excess * NOISE_EXCESS_GAIN * noise.uniform()
                    })
                });
                mix * NOISE_SINE_GAIN * self.window[n] * self.magnitude * noise_gain
            }
        }
    }
}
