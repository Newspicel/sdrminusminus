#![allow(clippy::expect_used)]

use num_complex::Complex;
use sdrmm_dsp::fft::Transform;

use crate::drm::{
    aac::{AudioConfig, AudioMode, Coding, MAX_CONFIG, drm_frame_padded},
    audio::{TEXT_BYTES, aac_frames, build_aac_superframe},
    cells::{Cell, Layout},
    coding::{Plan, Qam},
    fac::{Fac, FacService, fac_bits, fac_rate},
    mlc,
    mode::Robustness,
    msc::{CellInterleaver, MscConfig, plan as msc_plan},
    sdc::{self, Audio, Multiplex, Sdc, Stream},
    text,
};

pub const RATE_HZ: f64 = 192_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    HeMono,
    HeParametric,
    LcStereo,
    LcMono48,
}

impl Source {
    fn units(self) -> Vec<Vec<u8>> {
        let mut bytes: &[u8] = match self {
            Self::HeMono => include_bytes!("../../../../fixtures/drm/drm_he_mono_24k.aus"),
            Self::HeParametric => include_bytes!("../../../../fixtures/drm/drm_he_ps_24k.aus"),
            Self::LcStereo => include_bytes!("../../../../fixtures/drm/drm_lc_stereo_24k.aus"),
            Self::LcMono48 => include_bytes!("../../../../fixtures/drm/drm_lc_mono_48k.aus"),
        };
        let mut units = Vec::new();
        while bytes.len() >= 2 {
            let size = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
            units.push(bytes[2..2 + size].to_vec());
            bytes = &bytes[2 + size..];
        }
        units
    }

    #[must_use]
    pub fn config(self, text: bool) -> AudioConfig {
        let (sbr, mode, rate_hz, rate_code) = match self {
            Self::HeMono => (true, AudioMode::Mono, 12_000, 1),
            Self::HeParametric => (true, AudioMode::ParametricStereo, 12_000, 1),
            Self::LcStereo => (false, AudioMode::Stereo, 24_000, 3),
            Self::LcMono48 => (false, AudioMode::Mono, 48_000, 5),
        };
        AudioConfig {
            coding: Coding::Aac,
            sbr,
            mode,
            rate_hz,
            rate_code,
            text,
            surround: 0,
            config: [0; MAX_CONFIG],
            config_length: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Service {
    pub id: u32,
    pub label: &'static str,
    pub source: Source,
    pub text: Option<&'static str>,
    pub language: u8,
    pub programme: u8,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub mode: Robustness,
    pub occupancy: u8,
    pub msc: Qam,
    pub protection_higher: u8,
    pub protection_lower: u8,
    pub higher_bytes: u16,
    pub short_interleave: bool,
    pub sdc_robust: bool,
    pub services: Vec<Service>,
}

#[must_use]
pub fn music(mode: Robustness) -> Service {
    Service {
        id: 0x00_D7_A1,
        label: "Rust Wave",
        source: if mode.plus() {
            Source::LcMono48
        } else {
            Source::HeMono
        },
        text: Some("Now playing: a 700 Hz test tone"),
        language: 5,
        programme: 10,
    }
}

#[must_use]
pub fn talk() -> Service {
    Service {
        id: 0x00_D7_A2,
        label: "Rust Talk",
        source: Source::HeParametric,
        text: None,
        language: 7,
        programme: 1,
    }
}

#[must_use]
pub fn defaults(mode: Robustness) -> Config {
    let plus = mode.plus();
    Config {
        mode,
        occupancy: if plus { 0 } else { 3 },
        msc: if plus { Qam::Q16 } else { Qam::Q64 },
        protection_higher: 0,
        protection_lower: if plus { 2 } else { 1 },
        higher_bytes: 0,
        short_interleave: false,
        sdc_robust: true,
        services: vec![music(mode)],
    }
}

struct Program {
    config: AudioConfig,
    units: Vec<Vec<u8>>,
    next: usize,
    text: Vec<[u8; 4]>,
    text_at: usize,
    pending: Vec<u8>,
}

impl Program {
    fn text_piece(&mut self) -> [u8; 4] {
        if self.text.is_empty() {
            return [0; 4];
        }
        let piece = self.text[self.text_at % self.text.len()];
        self.text_at += 1;
        piece
    }

    fn superframe(&mut self, plus: bool, length: usize) -> Vec<u8> {
        let count = aac_frames(plus, self.config.rate_hz).expect("a DRM AAC rate");
        let raw: Vec<Vec<u8>> = (0..count)
            .map(|index| self.units[(self.next + index) % self.units.len()].clone())
            .collect();
        self.next += count;
        let mut frames: Vec<Vec<u8>> = raw
            .iter()
            .map(|unit| drm_frame_padded(unit, &self.config, 0).expect("a DRM AAC frame"))
            .collect();
        let used: usize =
            frames.iter().map(|frame| frame.len()).sum::<usize>() + (count - 1) * 12 / 8 + 1;
        let last = frames.len() - 1;
        let spare = length.saturating_sub(used);
        frames[last] = drm_frame_padded(&raw[last], &self.config, frames[last].len() - 1 + spare)
            .expect("a padded DRM AAC frame");
        build_aac_superframe(&frames, length).expect("the audio fits its stream")
    }

    fn logical(&mut self, plus: bool, length: usize, first: bool) -> Vec<u8> {
        let audio = length - if self.config.text { TEXT_BYTES } else { 0 };
        let mut frame = if plus {
            if first {
                let whole = self.superframe(plus, 2 * audio);
                self.pending = whole[audio..].to_vec();
                whole[..audio].to_vec()
            } else {
                std::mem::take(&mut self.pending)
            }
        } else {
            self.superframe(plus, audio)
        };
        if self.config.text {
            frame.extend_from_slice(&self.text_piece());
        }
        frame
    }
}

pub struct Transmitter {
    config: Config,
    layout: Layout,
    fac_plan: Plan,
    sdc_plan: Plan,
    sdc_blocks: Vec<Vec<bool>>,
    msc_plan: Plan,
    multiplex: Multiplex,
    programs: Vec<Program>,
    interleaver: CellInterleaver,
    logical_count: usize,
    fft: Transform,
    oversample: usize,
}

fn services_code(count: usize) -> u8 {
    match count {
        1 => 0b0100,
        2 => 0b1000,
        3 => 0b1100,
        _ => 0b0000,
    }
}

fn pack_sdc(entities: Vec<Sdc>, plan: &Plan) -> Vec<Vec<bool>> {
    let data_bytes = (plan.bits() - 20) / 8;
    let encode = |block: &Sdc| sdc::encode(block, data_bytes, plan.bits());
    let mut blocks = Vec::new();
    let mut current = Sdc::default();
    for entity in entities {
        let mut candidate = current.clone();
        candidate.merge(entity.clone());
        if encode(&candidate).is_some() {
            current = candidate;
        } else {
            blocks.push(encode(&current).expect("an SDC block"));
            current = entity;
        }
    }
    blocks.push(encode(&current).expect("an SDC entity fits a block"));
    blocks
}

impl Transmitter {
    #[must_use]
    pub fn new(config: Config) -> Self {
        let mode = config.mode;
        let plus = mode.plus();
        let layout = Layout::new(mode, config.occupancy).expect("a defined occupancy");
        let fac_plan = Plan::fac(Qam::Q4, mode.fac_cells(), fac_rate(plus), fac_bits(plus));
        let fac = Self::fac_template(&config, 0);
        let (sdc_qam, sdc_rates) = fac.sdc_coding();
        let sdc_plan = Plan::eep(sdc_qam, layout.sdc_cells, sdc_rates).expect("an SDC plan");
        let probe = Multiplex {
            protection_higher: config.protection_higher,
            protection_lower: config.protection_lower,
            streams: vec![Stream {
                higher: config.higher_bytes,
                lower: 0,
            }],
        };
        let msc_config = MscConfig {
            qam: config.msc,
            plus,
            cells: layout.multiplex_cells(),
        };
        let capacity = msc_plan(msc_config, &probe).expect("an MSC plan").bits() / 8;
        let count = config.services.len();
        let share = (capacity - usize::from(config.higher_bytes) * count) / count;
        let multiplex = Multiplex {
            protection_higher: config.protection_higher,
            protection_lower: config.protection_lower,
            streams: (0..count)
                .map(|_| Stream {
                    higher: config.higher_bytes,
                    lower: share as u16,
                })
                .collect(),
        };
        let msc = msc_plan(msc_config, &multiplex).expect("an MSC plan");
        let mut entities = vec![Sdc {
            multiplex: Some(multiplex.clone()),
            ..Sdc::default()
        }];
        let programs: Vec<Program> = config
            .services
            .iter()
            .enumerate()
            .map(|(index, service)| {
                let audio = service.source.config(service.text.is_some());
                let mut entry = Sdc::default();
                entry.audio[index] = Some(Audio {
                    stream: index as u8,
                    config: audio,
                });
                entities.push(entry);
                let mut entry = Sdc::default();
                entry.labels[index] = Some(service.label.to_owned());
                entities.push(entry);
                let mut entry = Sdc::default();
                entry.languages[index] = Some(("eng".to_owned(), "de".to_owned()));
                entities.push(entry);
                Program {
                    config: audio,
                    units: service.source.units(),
                    next: index * 7,
                    text: service
                        .text
                        .map(|message| text::pieces(message, false))
                        .unwrap_or_default(),
                    text_at: 0,
                    pending: Vec::new(),
                }
            })
            .collect();
        let sdc_blocks = pack_sdc(entities, &sdc_plan);
        let oversample = if plus { 1 } else { 4 };
        Self {
            interleaver: CellInterleaver::new(
                layout.multiplex_cells(),
                if config.short_interleave && !plus {
                    1
                } else {
                    mode.interleave_depth()
                },
            ),
            fft: Transform::inverse(mode.useful() * oversample),
            config,
            layout,
            fac_plan,
            sdc_plan,
            sdc_blocks,
            msc_plan: msc,
            multiplex,
            programs,
            logical_count: 0,
            oversample,
        }
    }

    fn fac_template(config: &Config, frame: usize) -> Fac {
        let plus = config.mode.plus();
        let (identity, toggle) = Fac::identity_for(plus, frame);
        Fac {
            enhancement: false,
            identity,
            plus,
            occupancy: config.occupancy,
            short_interleave: config.short_interleave && !plus,
            msc: config.msc,
            sdc_robust: config.sdc_robust,
            services: services_code(config.services.len()),
            reconfiguration: 0,
            toggle,
            service: [None, None],
        }
    }

    fn fac(&self, frame: usize, superframe: usize) -> Fac {
        let plus = self.config.mode.plus();
        let count = self.config.services.len();
        let per = if plus { 2 } else { 1 };
        let base = (superframe * self.config.mode.frames() + frame) * per;
        let service = |slot: usize| {
            let index = (base + slot) % count;
            let service = &self.config.services[index];
            FacService {
                id: service.id,
                short_id: index as u8,
                audio_ca: false,
                language: service.language,
                data: false,
                descriptor: service.programme,
                data_ca: false,
            }
        };
        Fac {
            service: [Some(service(0)), plus.then(|| service(1))],
            ..Self::fac_template(&self.config, frame)
        }
    }

    fn multiplex_frame(&mut self) -> Vec<bool> {
        let plus = self.config.mode.plus();
        let first = self.logical_count.is_multiple_of(2);
        self.logical_count += 1;
        let mut higher = Vec::new();
        let mut lower = Vec::new();
        for (program, stream) in self.programs.iter_mut().zip(&self.multiplex.streams) {
            let length = usize::from(stream.higher + stream.lower);
            let logical = program.logical(plus, length, first);
            higher.extend_from_slice(&logical[..usize::from(stream.higher)]);
            lower.extend_from_slice(&logical[usize::from(stream.higher)..]);
        }
        higher.extend_from_slice(&lower);
        crate::drm::bits::byte_bits(&higher).collect()
    }

    fn superframe_cells(&mut self) -> Vec<Complex<f32>> {
        let mode = self.config.mode;
        let mut cells = Vec::with_capacity(self.layout.msc_cells);
        for _ in 0..mode.frames() {
            let bits = self.multiplex_frame();
            let frame = mlc::encode(&self.msc_plan, &bits);
            cells.extend(self.interleaver.push(frame));
        }
        let dummy = self.msc_plan.qam.scale();
        let mut sign = 1.0;
        while cells.len() < self.layout.msc_cells {
            cells.push(Complex::new(dummy, sign * dummy));
            sign = -sign;
        }
        cells
    }

    fn symbol(&mut self, carriers: &[(i32, Complex<f32>)], out: &mut Vec<Complex<f32>>) {
        let mode = self.config.mode;
        let size = mode.useful() * self.oversample;
        let guard = mode.guard() * self.oversample;
        let mut buffer = vec![Complex::default(); size];
        for &(k, value) in carriers {
            buffer[k.rem_euclid(size as i32) as usize] = value;
        }
        self.fft.process(&mut buffer);
        let scale = (mode.useful() as f32).sqrt().recip() * 0.25;
        out.extend(buffer[size - guard..].iter().map(|&value| value * scale));
        out.extend(buffer.iter().map(|&value| value * scale));
    }

    pub fn superframe(&mut self, index: usize, out: &mut Vec<Complex<f32>>) {
        let mode = self.config.mode;
        let msc = self.superframe_cells();
        let block = &self.sdc_blocks[index % self.sdc_blocks.len()];
        let sdc = mlc::encode(&self.sdc_plan, block);
        let mut msc_at = 0;
        let mut sdc_at = 0;
        let mut carriers = Vec::with_capacity(self.layout.width());
        for frame in 0..mode.frames() {
            let fac_bits = self.fac(frame, index).encode();
            let fac = mlc::encode(&self.fac_plan, &fac_bits);
            let mut fac_at = 0;
            for symbol in 0..mode.symbols() {
                carriers.clear();
                for (offset, cell) in self.layout.symbol(frame, symbol).iter().enumerate() {
                    let k = self.layout.low + offset as i32;
                    let value = match *cell {
                        Cell::Unused => continue,
                        Cell::Pilot { value, .. } => value,
                        Cell::Fac => {
                            fac_at += 1;
                            fac[fac_at - 1]
                        }
                        Cell::Sdc => {
                            sdc_at += 1;
                            sdc[sdc_at - 1]
                        }
                        Cell::Msc => {
                            msc_at += 1;
                            msc[msc_at - 1]
                        }
                    };
                    carriers.push((k, value));
                }
                self.symbol(&carriers, out);
            }
        }
    }
}

#[must_use]
pub fn signal(config: Config, superframes: usize) -> Vec<Complex<f32>> {
    let mut transmitter = Transmitter::new(config);
    let mut out = Vec::new();
    for index in 0..superframes {
        transmitter.superframe(index, &mut out);
    }
    out
}

#[must_use]
pub fn multipath(iq: &[Complex<f32>], taps: &[(f64, Complex<f32>)]) -> Vec<Complex<f32>> {
    let mut out = vec![Complex::default(); iq.len()];
    for &(delay_s, gain) in taps {
        let delay = (delay_s * RATE_HZ).round() as usize;
        for (index, value) in out.iter_mut().enumerate().skip(delay) {
            *value += iq[index - delay] * gain;
        }
    }
    out
}
