use num_complex::Complex;

use super::{
    aac::{AudioConfig, Coding},
    audio::{Superframes, XheAssembler, aac_frames, aac_superframe, split_text},
    bits::pack,
    cells::{Cell as Kind, Layout},
    coding::{Plan, Qam},
    fac::{Fac, fac_bits, fac_rate},
    mlc::MlcDecoder,
    mode::Robustness,
    msc::{Cell, CellDeinterleaver, MscConfig, plan as msc_plan, stream},
    sdc::Sdc,
    text::TextMessage,
};
use crate::broadcast_media::BroadcastMedia;

const SNR_SMOOTHING: f32 = 0.8;
const FAC_HOLD: u32 = 4;

pub struct Selection {
    pub short_id: u8,
    pub stream: usize,
    pub config: AudioConfig,
}

pub struct Receiver {
    pub mode: Robustness,
    width: usize,
    low: i32,
    cells: Vec<Complex<f32>>,
    weights: Vec<f32>,
    fac_plan: Plan,
    fac_cells: Vec<Complex<f32>>,
    fac_weights: Vec<f32>,
    fac_bits: Vec<bool>,
    decoder: MlcDecoder,
    bits: Vec<bool>,
    bytes: Vec<u8>,
    pub layout: Option<Layout>,
    pub fac: Option<Fac>,
    pub services: [Option<super::fac::FacService>; 4],
    frame: Option<usize>,
    pub fac_ok: u32,
    pub fac_bad: u32,
    fac_misses: u32,
    pub sdc: Sdc,
    pub sdc_ok: u32,
    pub sdc_bad: u32,
    sdc_cells: Vec<Cell>,
    msc: Vec<Cell>,
    multiplex_index: usize,
    superframe_valid: bool,
    deinterleaver: Option<CellDeinterleaver>,
    deinterleaved: Vec<Cell>,
    values: Vec<Complex<f32>>,
    gains: Vec<f32>,
    logical: Vec<u8>,
    audio: Vec<u8>,
    unit: Vec<u8>,
    ranges: Vec<(u8, std::ops::Range<usize>)>,
    xhe_frames: Vec<Vec<u8>>,
    pub selection: Option<Selection>,
    pub wanted: Option<u8>,
    pub changed: bool,
    superframes: Superframes,
    xhe: XheAssembler,
    pub text: TextMessage,
    pairing_phase: usize,
    pairing_failures: u32,
    pub snr_db: f32,
    pub logical_frames: u32,
    pub audio_superframes: u32,
}

impl Receiver {
    #[must_use]
    pub fn new(mode: Robustness) -> Self {
        let (low, high) = mode.span();
        let width = (high - low + 1) as usize;
        let symbols = mode.symbols();
        Self {
            mode,
            width,
            low,
            cells: vec![Complex::default(); symbols * width],
            weights: vec![0.0; symbols * width],
            fac_plan: Plan::fac(
                Qam::Q4,
                mode.fac_cells(),
                fac_rate(mode.plus()),
                fac_bits(mode.plus()),
            ),
            fac_cells: Vec::with_capacity(mode.fac_cells()),
            fac_weights: Vec::with_capacity(mode.fac_cells()),
            fac_bits: Vec::with_capacity(fac_bits(mode.plus())),
            decoder: MlcDecoder::default(),
            bits: Vec::new(),
            bytes: Vec::new(),
            layout: None,
            fac: None,
            services: [None; 4],
            frame: None,
            fac_ok: 0,
            fac_bad: 0,
            fac_misses: FAC_HOLD,
            sdc: Sdc::default(),
            sdc_ok: 0,
            sdc_bad: 0,
            sdc_cells: Vec::new(),
            msc: Vec::new(),
            multiplex_index: 0,
            superframe_valid: false,
            deinterleaver: None,
            deinterleaved: Vec::new(),
            values: Vec::new(),
            gains: Vec::new(),
            logical: Vec::new(),
            audio: Vec::new(),
            unit: Vec::new(),
            ranges: Vec::new(),
            xhe_frames: Vec::new(),
            selection: None,
            wanted: None,
            changed: false,
            superframes: Superframes::new(mode.plus()),
            xhe: XheAssembler::default(),
            text: TextMessage::default(),
            pairing_phase: 0,
            pairing_failures: 0,
            snr_db: 0.0,
            logical_frames: 0,
            audio_superframes: 0,
        }
    }

    #[must_use]
    pub fn fac_locked(&self) -> bool {
        self.fac_misses < FAC_HOLD
    }

    pub fn store(&mut self, symbol: usize, equalized: &[Complex<f32>], weights: &[f32]) {
        let start = symbol * self.width;
        self.cells[start..start + self.width].copy_from_slice(equalized);
        self.weights[start..start + self.width].copy_from_slice(weights);
    }

    fn cell(&self, symbol: usize, k: i32) -> (Complex<f32>, f32) {
        let index = symbol * self.width + (k - self.low) as usize;
        (self.cells[index], self.weights[index])
    }

    fn decode_fac(&mut self) -> Option<Fac> {
        self.fac_cells.clear();
        self.fac_weights.clear();
        for symbol in 0..self.mode.symbols() {
            for &k in self.mode.fac_carriers(symbol) {
                let (value, weight) = self.cell(symbol, k);
                self.fac_cells.push(value);
                self.fac_weights.push(weight);
            }
        }
        self.decoder.decode(
            &self.fac_plan,
            &self.fac_cells,
            &self.fac_weights,
            &mut self.fac_bits,
        )?;
        let fac = Fac::parse(&self.fac_bits, self.mode.plus())?;
        self.measure_snr();
        Some(fac)
    }

    fn measure_snr(&mut self) {
        let scale = Qam::Q4.scale();
        let (mut signal, mut noise) = (0.0f32, 0.0f32);
        for (&cell, &weight) in self.fac_cells.iter().zip(&self.fac_weights) {
            let ideal = Complex::new(scale.copysign(cell.re), scale.copysign(cell.im));
            signal += weight * ideal.norm_sqr();
            noise += weight * (cell - ideal).norm_sqr();
        }
        let snr = 10.0 * (signal / noise.max(f32::EPSILON)).log10();
        self.snr_db = if self.snr_db == 0.0 {
            snr
        } else {
            SNR_SMOOTHING * self.snr_db + (1.0 - SNR_SMOOTHING) * snr
        }
        .clamp(-10.0, 60.0);
    }

    fn adopt(&mut self, fac: Fac) -> bool {
        let changed = self
            .layout
            .as_ref()
            .is_none_or(|layout| layout.occupancy != fac.occupancy);
        if changed {
            self.layout = Layout::new(self.mode, fac.occupancy);
            self.reset_msc();
        }
        for service in fac.service.iter().flatten() {
            self.services[usize::from(service.short_id)] = Some(*service);
        }
        self.fac = Some(fac);
        changed
    }

    pub fn reset_msc(&mut self) {
        self.msc.clear();
        self.superframe_valid = false;
        self.deinterleaver = None;
        self.superframes.reset();
        self.xhe.reset();
    }

    pub fn frame(&mut self, media: &mut BroadcastMedia) -> Option<u8> {
        let decoded = self.decode_fac();
        let mut occupancy = None;
        let index = match decoded {
            Some(fac) => {
                self.fac_ok = self.fac_ok.saturating_add(1);
                self.fac_misses = 0;
                let index = fac.frame();
                if self.adopt(fac) {
                    occupancy = Some(fac.occupancy);
                }
                Some(index)
            }
            None => {
                self.fac_bad = self.fac_bad.saturating_add(1);
                self.fac_misses = self.fac_misses.saturating_add(1);
                self.frame.map(|frame| (frame + 1) % self.mode.frames())
            }
        };
        self.frame = index;
        let Some(index) = index else {
            return occupancy;
        };
        if self.fac.is_none() || self.layout.is_none() {
            return occupancy;
        }
        if index == 0 {
            self.decode_sdc();
            self.msc.clear();
            self.multiplex_index = 0;
            self.superframe_valid = true;
        }
        if self.superframe_valid {
            self.collect_msc(index, media);
        }
        occupancy
    }

    fn decode_sdc(&mut self) {
        let (Some(fac), Some(layout)) = (self.fac, self.layout.as_ref()) else {
            return;
        };
        self.sdc_cells.clear();
        for symbol in 0..self.mode.sdc_symbols() {
            for (offset, kind) in layout.symbol(0, symbol).iter().enumerate() {
                if matches!(kind, Kind::Sdc) {
                    let (value, weight) = self.cell(symbol, layout.low + offset as i32);
                    self.sdc_cells.push(Cell { value, weight });
                }
            }
        }
        let (qam, rates) = fac.sdc_coding();
        let Some(plan) = Plan::eep(qam, self.sdc_cells.len(), rates) else {
            return;
        };
        self.values.clear();
        self.gains.clear();
        self.values
            .extend(self.sdc_cells.iter().map(|cell| cell.value));
        self.gains
            .extend(self.sdc_cells.iter().map(|cell| cell.weight));
        let data_bytes = (plan.bits() - 20) / 8;
        let parsed = self
            .decoder
            .decode(&plan, &self.values, &self.gains, &mut self.bits)
            .and_then(|()| Sdc::parse(&self.bits, data_bytes));
        match parsed {
            Some(update) => {
                self.sdc_ok = self.sdc_ok.saturating_add(1);
                self.sdc.merge(update);
            }
            None => self.sdc_bad = self.sdc_bad.saturating_add(1),
        }
    }

    fn collect_msc(&mut self, index: usize, media: &mut BroadcastMedia) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        for symbol in 0..self.mode.symbols() {
            let row = layout.symbol(index, symbol);
            for (offset, kind) in row.iter().enumerate() {
                if matches!(kind, Kind::Msc) {
                    let at = symbol * self.width + (layout.low + offset as i32 - self.low) as usize;
                    self.msc.push(Cell {
                        value: self.cells[at],
                        weight: self.weights[at],
                    });
                }
            }
        }
        let per = layout.multiplex_cells();
        while self.msc.len() >= (self.multiplex_index + 1) * per {
            let start = self.multiplex_index * per;
            let position = self.multiplex_index;
            self.multiplex_index += 1;
            let cells = std::mem::take(&mut self.msc);
            self.multiplex(&cells[start..start + per], position, media);
            self.msc = cells;
        }
    }

    fn multiplex(&mut self, frame: &[Cell], position: usize, media: &mut BroadcastMedia) {
        let Some(fac) = self.fac else {
            return;
        };
        let depth = fac.interleave_depth(self.mode);
        if self
            .deinterleaver
            .as_ref()
            .is_none_or(|deinterleaver| !deinterleaver.matches(frame.len(), depth))
        {
            self.deinterleaver = Some(CellDeinterleaver::new(frame.len(), depth));
        }
        let Some(deinterleaver) = self.deinterleaver.as_mut() else {
            return;
        };
        let mut deinterleaved = std::mem::take(&mut self.deinterleaved);
        let ready = deinterleaver.push(frame, &mut deinterleaved);
        if ready {
            let logical =
                (position + self.mode.frames() * depth - (depth - 1)) % self.mode.frames();
            self.decode_multiplex(&deinterleaved, logical, media);
        }
        self.deinterleaved = deinterleaved;
    }

    fn decode_multiplex(&mut self, cells: &[Cell], logical: usize, media: &mut BroadcastMedia) {
        let (Some(fac), Some(multiplex)) = (self.fac, self.sdc.multiplex.as_ref()) else {
            return;
        };
        let config = MscConfig {
            qam: fac.msc,
            plus: fac.plus,
            cells: cells.len(),
        };
        let Some(plan) = msc_plan(config, multiplex) else {
            return;
        };
        self.values.clear();
        self.gains.clear();
        self.values.extend(cells.iter().map(|cell| cell.value));
        self.gains.extend(cells.iter().map(|cell| cell.weight));
        if self
            .decoder
            .decode(&plan, &self.values, &self.gains, &mut self.bits)
            .is_none()
        {
            return;
        }
        pack(&self.bits, &mut self.bytes);
        self.logical_frames = self.logical_frames.saturating_add(1);
        self.select();
        let Some(selection) = self.selection.as_ref() else {
            return;
        };
        let stream_index = selection.stream;
        let mut logical_bytes = std::mem::take(&mut self.logical);
        let found = self.sdc.multiplex.as_ref().is_some_and(|multiplex| {
            stream(&self.bytes, multiplex, stream_index, &mut logical_bytes).is_some()
        });
        if found {
            self.play(&logical_bytes, logical, media);
        }
        self.logical = logical_bytes;
    }

    fn select(&mut self) {
        let wanted = self.wanted;
        let chosen = (0..4u8)
            .filter(|&id| wanted.is_none_or(|wanted| wanted == id))
            .find_map(|id| {
                let audio = self.sdc.audio[usize::from(id)]?;
                let service = self.services[usize::from(id)];
                if service.is_some_and(|service| service.data || service.audio_ca) {
                    return None;
                }
                Some(Selection {
                    short_id: id,
                    stream: usize::from(audio.stream),
                    config: audio.config,
                })
            });
        let same = match (&self.selection, &chosen) {
            (Some(current), Some(next)) => {
                current.short_id == next.short_id
                    && current.stream == next.stream
                    && current.config == next.config
            }
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.superframes.reset();
            self.xhe.reset();
            self.text.reset();
            self.selection = chosen;
            self.changed = true;
        }
    }

    fn play(&mut self, logical: &[u8], index: usize, media: &mut BroadcastMedia) {
        let Some(selection) = self.selection.as_ref() else {
            return;
        };
        let config = selection.config;
        let (audio, text) = split_text(logical, &config);
        if let Some(piece) = text {
            self.text.push(piece);
        }
        let starts = (index + self.pairing_phase).is_multiple_of(2);
        let mut superframe = std::mem::take(&mut self.audio);
        if self.superframes.push(audio, starts, &mut superframe) {
            self.audio_superframes = self.audio_superframes.saturating_add(1);
            match config.coding {
                Coding::Aac => self.aac(&superframe, &config, media),
                Coding::Xhe => self.xhe_superframe(&superframe, &config, media),
            }
        }
        self.audio = superframe;
    }

    fn aac(&mut self, superframe: &[u8], config: &AudioConfig, media: &mut BroadcastMedia) {
        let Some(count) = aac_frames(self.mode.plus(), config.rate_hz) else {
            media.audio_gap(1, "Unsupported DRM AAC sampling rate");
            return;
        };
        let mut ranges = std::mem::take(&mut self.ranges);
        match aac_superframe(superframe, count, &mut ranges) {
            Ok(()) => {
                self.pairing_failures = 0;
                for (check, range) in &ranges {
                    self.unit.clear();
                    self.unit.push(*check);
                    self.unit.extend_from_slice(&superframe[range.clone()]);
                    media.push_drm(&self.unit, *config);
                }
            }
            Err(reason) => {
                media.audio_gap(count as u32, reason);
                self.pairing_failures += 1;
                if self.mode.plus() && self.pairing_failures >= 2 {
                    self.pairing_phase ^= 1;
                    self.pairing_failures = 0;
                    self.superframes.reset();
                }
            }
        }
        self.ranges = ranges;
    }

    fn xhe_superframe(
        &mut self,
        superframe: &[u8],
        config: &AudioConfig,
        media: &mut BroadcastMedia,
    ) {
        let mut frames = std::mem::take(&mut self.xhe_frames);
        match self.xhe.push(superframe, &mut frames) {
            Ok(errors) => {
                if errors > 0 {
                    media.audio_gap(errors, "xHE-AAC frame CRC failure");
                }
                for frame in &frames {
                    media.push_drm(frame, *config);
                }
            }
            Err(reason) => media.audio_gap(1, reason),
        }
        self.xhe_frames = frames;
    }
}
