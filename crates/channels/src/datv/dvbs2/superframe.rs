use num_complex::Complex;

use codes::{Gold, Search, Sequence};
use layout::{CU, HEADER, PERIOD, PILOT_START, TYPE_A};
use signalling::{Header, Protection};
use tracker::{Mark, Tracker};

use super::pl;

mod codes;
pub mod coding;
mod detect;
pub mod layout;
pub mod signalling;
#[cfg(any(test, feature = "synth"))]
pub mod synth;
mod tracker;

pub use layout::LENGTH;
pub use tracker::Burst;

const PHASE_GAIN: f32 = 0.5;
const FREQUENCY_GAIN: f32 = 0.1;
const HEADER_CONFIDENCE: f32 = 0.3;
const BUNDLE_CONFIDENCE: f32 = 0.3;
const POSTAMBLE_COHERENCE: f32 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Plain,
    Legacy,
    Extended,
    Unsupported(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Symbols(Content),
    Burst(Burst),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub reference: u32,
    pub payload: u32,
    pub search: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub format: Option<u8>,
    pub sosf: u8,
    pub pilot: Option<u8>,
    pub trailer: Option<u8>,
    pub reference: u32,
    pub payload: u32,
}

#[derive(Clone, Copy, Debug)]
struct Active {
    format: u8,
    content: Content,
    position: usize,
    end: usize,
    pilots: bool,
    phase: f32,
    frequency: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Piece {
    Header,
    Trailer,
    Extension,
    Indication,
    Pilot,
    Unit,
    Run,
    Tail,
}

pub struct Container {
    pending: Vec<Complex<f32>>,
    offset: usize,
    active: Option<Active>,
    gold: Gold,
    reference: Sequence,
    payload: Sequence,
    search: Option<Search>,
    settings: Settings,
    identity: Identity,
    pilot_votes: [f32; 32],
    trailer_votes: [f32; 64],
    tracker: Tracker,
    collected: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    pilots: bool,
    protection: Protection,
    scanned: usize,
    dropped: u32,
}

fn wrapped(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

impl Container {
    #[must_use]
    pub fn new() -> Self {
        let gold = Gold::new();
        Self {
            pending: Vec::with_capacity(8192),
            offset: 0,
            active: None,
            reference: Sequence::new(&gold, 0),
            payload: Sequence::new(&gold, 0),
            gold,
            search: None,
            settings: Settings::default(),
            identity: Identity::default(),
            pilot_votes: [0.0; 32],
            trailer_votes: [0.0; 64],
            tracker: Tracker::new(),
            collected: Vec::with_capacity(layout::LONG.stream()),
            scratch: Vec::with_capacity(HEADER),
            pilots: true,
            protection: Protection::Standard,
            scanned: usize::MAX / 2,
            dropped: 0,
        }
    }

    pub fn configure(&mut self, settings: Settings) {
        if settings == self.settings {
            return;
        }
        self.settings = settings;
        self.reference.set(&self.gold, settings.reference);
        self.payload.set(&self.gold, settings.payload);
        if settings.search && self.search.is_none() {
            self.search = Some(Search::new(&self.gold));
        }
        if !settings.search {
            self.search = None;
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.offset = 0;
        self.active = None;
        self.identity = Identity {
            reference: self.reference.code(),
            payload: self.payload.code(),
            ..Identity::default()
        };
        self.pilot_votes = [0.0; 32];
        self.trailer_votes = [0.0; 64];
        self.tracker.reset();
        self.collected.clear();
        self.pilots = true;
        self.protection = Protection::Standard;
        self.scanned = usize::MAX / 2;
    }

    #[must_use]
    pub const fn identity(&self) -> Identity {
        self.identity
    }

    pub fn take_dropped(&mut self) -> u32 {
        let dropped = self.dropped + self.tracker.dropped;
        self.dropped = 0;
        self.tracker.dropped = 0;
        dropped
    }

    pub fn push(&mut self, symbols: &[Complex<f32>]) {
        if self.offset > 0 {
            self.pending.drain(..self.offset);
            self.offset = 0;
        }
        self.pending.extend_from_slice(symbols);
    }

    pub fn next(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Unit> {
        out.clear();
        loop {
            if let Some(active) = self.active {
                if !(2..=7).contains(&active.format) {
                    return self.extract(out).map(Unit::Symbols);
                }
                if let Some(unit) = self.advance(out) {
                    return Some(unit);
                }
                if self.active.is_some() {
                    return None;
                }
            }
            if !self.scan(out)? {
                return Some(Unit::Symbols(Content::Plain));
            }
        }
    }

    fn scan(&mut self, out: &mut Vec<Complex<f32>>) -> Option<bool> {
        let available = self.pending.len() - self.offset;
        if available < HEADER {
            return None;
        }
        let mut position = self.offset;
        while position + HEADER <= self.pending.len() {
            if let Some(found) = self.detect_at(position) {
                if position > self.offset {
                    out.extend_from_slice(&self.pending[self.offset..position]);
                    self.scanned += position - self.offset;
                    self.offset = position;
                    return Some(false);
                }
                if available < PILOT_START + pl::PILOT_LENGTH {
                    return None;
                }
                self.scanned = 0;
                self.start(found);
                return Some(true);
            }
            position += 1;
        }
        out.extend_from_slice(&self.pending[self.offset..position]);
        self.scanned += position - self.offset;
        self.offset = position;
        Some(false)
    }

    fn detect_at(&mut self, position: usize) -> Option<detect::Found> {
        let samples = &self.pending[position..position + HEADER];
        if let Some(found) = detect::detect(samples, &self.reference, &self.payload) {
            return Some(found);
        }
        if self.scanned < LENGTH + HEADER {
            return None;
        }
        let search = self.search.as_ref()?;
        let (reference, payload) = detect::identify(&self.gold, search, samples)?;
        if reference != self.reference.code() || payload != self.payload.code() {
            self.reference.set(&self.gold, reference);
            self.payload.set(&self.gold, payload);
        }
        detect::detect(
            &self.pending[position..position + HEADER],
            &self.reference,
            &self.payload,
        )
    }

    fn start(&mut self, found: detect::Found) {
        self.identity.format = Some(found.format);
        self.identity.sosf = found.sosf;
        self.identity.reference = self.reference.code();
        self.identity.payload = self.payload.code();
        let mut active = Active {
            format: found.format,
            content: match found.format {
                0 => Content::Extended,
                1 => Content::Legacy,
                other => Content::Unsupported(other),
            },
            position: HEADER,
            end: if layout::fixed_length(found.format) || found.format > 7 {
                LENGTH
            } else {
                usize::MAX
            },
            pilots: true,
            phase: found.phase,
            frequency: found.frequency,
        };
        if found.format <= 1 {
            let start = self.offset + PILOT_START;
            active.pilots = self.legacy_pilots(start, &active).is_some();
        }
        if found.format == 7 {
            self.tracker.begin(
                7,
                Mark {
                    first: self.tracker.total(),
                    protection: Protection::Standard,
                    pointer: None,
                },
            );
        }
        self.tracker.tracked =
            layout::always_pilots(found.format) || layout::bundles(found.format).is_some();
        self.collected.clear();
        self.active = Some(active);
        self.offset += HEADER;
    }

    fn begin(&mut self, mark: Mark) {
        if let Some(active) = self.active {
            self.tracker.begin(active.format, mark);
        }
    }

    fn rotation(active: &Active, position: usize) -> Complex<f32> {
        Complex::from_polar(1.0, -active.phase - active.frequency * position as f32)
    }

    fn legacy_pilots(&mut self, start: usize, active: &Active) -> Option<u8> {
        let field = &self.pending[start..start + pl::PILOT_LENGTH];
        let (row, coherence) = detect::pilot_row(field, |k| {
            self.reference.known(PILOT_START + k, false)
                * Self::rotation(active, PILOT_START + k).conj()
        });
        self.pilot_votes[usize::from(row)] += coherence;
        if coherence < 0.5 {
            return None;
        }
        let mut reference = [Complex::new(0.0, 0.0); pl::PILOT_LENGTH];
        for (k, slot) in reference.iter_mut().enumerate() {
            *slot = self.reference.known(PILOT_START + k, codes::pilot(row, k));
        }
        detect::fit(field, &reference, active.frequency)?;
        self.identity.pilot = Some(row);
        Some(row)
    }

    fn extract(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Content> {
        let active = self.active.as_mut()?;
        let count = (self.pending.len() - self.offset).min(LENGTH - active.position);
        if count == 0 {
            return None;
        }
        let unsupported = matches!(active.content, Content::Unsupported(_));
        for i in 0..count {
            let position = active.position + i;
            let pilot = active.pilots && TYPE_A.inside(position);
            if !pilot && !unsupported {
                out.push(
                    self.pending[self.offset + i]
                        * Complex::from_polar(
                            1.0,
                            -active.phase - active.frequency * position as f32,
                        ),
                );
            }
        }
        let content = active.content;
        active.position += count;
        self.offset += count;
        if active.position == LENGTH {
            self.active = None;
        }
        Some(content)
    }

    fn piece(&self, active: &Active) -> (Piece, usize) {
        let position = active.position;
        if let Some(bundles) = layout::bundles(active.format) {
            let tail = bundles.tail();
            if position >= tail {
                return (Piece::Tail, LENGTH - position);
            }
            if bundles.grid.starts(position) {
                return (Piece::Pilot, bundles.grid.length);
            }
            let run = bundles.grid.next(position).min(tail) - position;
            return (Piece::Run, run.min(bundles.stream() - self.collected.len()));
        }
        match (active.format, position) {
            (4 | 5, HEADER) => (Piece::Header, signalling::header_symbols(active.format)),
            (4, signalling::TRAILER_START) => (Piece::Trailer, CU),
            (6, HEADER) => (Piece::Extension, 504),
            (6, _) if position < PILOT_START => (Piece::Indication, 216),
            _ if active.pilots && TYPE_A.starts(position) => (Piece::Pilot, pl::PILOT_LENGTH),
            _ => (Piece::Unit, CU),
        }
    }

    fn advance(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Unit> {
        loop {
            let active = self.active?;
            if active.position >= active.end {
                self.active = None;
                return None;
            }
            let (piece, length) = self.piece(&active);
            let boundary = active.format == 5
                && active.position > PILOT_START
                && active.position.is_multiple_of(PERIOD);
            let truncated = active.format == 5 && piece == Piece::Unit;
            let needed = if boundary { length.max(HEADER) } else { length };
            if self.pending.len() - self.offset < needed {
                return None;
            }
            if (boundary && self.sosf_here()) || (truncated && self.postamble(&active)) {
                self.active = None;
                return None;
            }
            let unit = self.process(piece, length, out);
            if let Some(active) = self.active.as_mut() {
                active.position += length;
            }
            self.offset += length;
            if self.finished() {
                self.active = None;
            }
            if unit.is_some() {
                return unit;
            }
        }
    }

    fn finished(&self) -> bool {
        self.active.is_some_and(|active| {
            matches!(active.format, 6 | 7)
                && active.position >= layout::payload_start(active.format)
                && (self.tracker.ended || self.tracker.lost())
        })
    }

    fn sosf_here(&self) -> bool {
        detect::detect(
            &self.pending[self.offset..self.offset + HEADER],
            &self.reference,
            &self.payload,
        )
        .is_some()
    }

    fn postamble(&self, active: &Active) -> bool {
        let bits = signalling::postamble();
        let mut sum = Complex::new(0.0, 0.0);
        let mut power = 0.0f32;
        for (k, &bit) in bits.iter().take(CU).enumerate() {
            let position = active.position + k;
            let symbol = self
                .payload
                .descramble(position, self.pending[self.offset + k])
                * Self::rotation(active, position);
            sum += symbol * signalling::bpsk(bit).conj();
            power += symbol.norm_sqr();
        }
        sum.norm() / (power * CU as f32).sqrt().max(1e-12) > POSTAMBLE_COHERENCE
    }

    fn take(&mut self, length: usize, reference: bool) {
        let Some(active) = self.active else {
            return;
        };
        self.scratch.clear();
        let sequence = if reference {
            &self.reference
        } else {
            &self.payload
        };
        for k in 0..length {
            let position = active.position + k;
            self.scratch.push(
                sequence.descramble(position, self.pending[self.offset + k])
                    * Self::rotation(&active, position),
            );
        }
    }

    fn process(
        &mut self,
        piece: Piece,
        length: usize,
        out: &mut Vec<Complex<f32>>,
    ) -> Option<Unit> {
        match piece {
            Piece::Header => self.header(length),
            Piece::Trailer => self.trailer(),
            Piece::Extension | Piece::Tail => {}
            Piece::Indication => self.indication(length),
            Piece::Pilot => self.pilot(length),
            Piece::Unit => return self.unit(out),
            Piece::Run => return self.run(length, out),
        }
        None
    }

    fn header(&mut self, length: usize) {
        let Some(active) = self.active else {
            return;
        };
        self.take(length, false);
        let read = signalling::read_header(&self.scratch, active.format)
            .filter(|(_, confidence)| *confidence >= HEADER_CONFIDENCE)
            .map(|(header, _)| header);
        let header = read.unwrap_or(Header {
            pointer: 0,
            pilots: self.pilots,
            protection: self.protection,
            system: 0,
        });
        self.pilots = header.pilots;
        self.protection = header.protection;
        if let Some(active) = self.active.as_mut() {
            active.pilots = header.pilots;
        }
        self.tracker.tracked = header.pilots;
        let first = self.tracker.total();
        let pointer = (u64::from(header.pointer) >= layout::first_unit(active.format))
            .then(|| first + u64::from(header.pointer) - layout::first_unit(active.format));
        self.begin(Mark {
            first,
            protection: header.protection,
            pointer,
        });
    }

    fn trailer(&mut self) {
        self.take(CU, true);
        let mut values = [Complex::new(0.0, 0.0); 64];
        values.copy_from_slice(&self.scratch[..64]);
        let constant = pl::pilot_symbol().conj();
        values.iter_mut().for_each(|value| *value *= constant);
        codes::hadamard(&mut values);
        for (vote, value) in self.trailer_votes.iter_mut().zip(&values) {
            *vote += value.norm_sqr();
        }
        self.identity.trailer = Some(best(&self.trailer_votes) as u8);
    }

    fn indication(&mut self, length: usize) {
        self.take(length, false);
        let protection = signalling::read_indication(&self.scratch)
            .filter(|(_, confidence)| *confidence >= HEADER_CONFIDENCE)
            .map_or(self.protection, |(protection, _)| protection);
        self.protection = protection;
        self.begin(Mark {
            first: self.tracker.total(),
            protection,
            pointer: None,
        });
    }

    fn pilot(&mut self, length: usize) {
        let Some(active) = self.active else {
            return;
        };
        self.take(length, true);
        let short = layout::bundles(active.format).is_some_and(|bundles| bundles.short_pilots);
        let constant = pl::pilot_symbol().conj();
        let mut values = [Complex::new(0.0, 0.0); 32];
        for (value, &symbol) in values.iter_mut().zip(&self.scratch) {
            *value = symbol * constant;
        }
        codes::hadamard(&mut values);
        for (vote, value) in self.pilot_votes.iter_mut().zip(&values) {
            *vote += value.norm_sqr();
        }
        let row = best(&self.pilot_votes) as u8;
        self.identity.pilot = Some(row);
        let sum =
            self.scratch
                .iter()
                .enumerate()
                .fold(Complex::new(0.0, 0.0), |sum, (k, &symbol)| {
                    let negative = if short {
                        codes::short_pilot(row, k)
                    } else {
                        codes::pilot(row, k)
                    };
                    sum + symbol * signalling::bpsk(negative).conj()
                });
        let period = layout::bundles(active.format).map_or(PERIOD, |bundles| bundles.grid.period);
        self.steer(sum.arg(), active.position + length / 2, period);
    }

    fn steer(&mut self, error: f32, center: usize, period: usize) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let error = wrapped(error);
        let step = FREQUENCY_GAIN * error / period as f32;
        active.phase += PHASE_GAIN * error - step * center as f32;
        active.frequency += step;
    }

    fn unit(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Unit> {
        let active = self.active?;
        self.take(CU, false);
        self.tracker.push(&self.scratch);
        let burst = self.tracker.poll(out);
        if let Some(error) = self.tracker.error.take() {
            self.steer(error, active.position, 16 * PERIOD);
        }
        burst.map(Unit::Burst)
    }

    fn run(&mut self, length: usize, out: &mut Vec<Complex<f32>>) -> Option<Unit> {
        let active = self.active?;
        let bundles = layout::bundles(active.format)?;
        self.take(length, false);
        self.collected.extend_from_slice(&self.scratch);
        if self.collected.len() < bundles.stream() {
            return None;
        }
        let burst = self.bundle(active.format, bundles, out);
        self.collected.clear();
        burst
    }

    fn bundle(
        &mut self,
        format: u8,
        bundles: layout::Bundles,
        out: &mut Vec<Complex<f32>>,
    ) -> Option<Unit> {
        let Some((code, confidence)) =
            signalling::read_bundle_header(&self.collected, bundles.replicas)
        else {
            self.dropped += 1;
            return None;
        };
        if confidence < BUNDLE_CONFIDENCE {
            self.dropped += 1;
            return None;
        }
        let coding = match coding::bundle(format, code) {
            Some(coding::Signal::Data(coding)) => coding,
            Some(coding::Signal::Dummy) => return None,
            None => {
                self.dropped += 1;
                return None;
            }
        };
        let Some(frames) = coding.bundled(bundles.payload) else {
            self.dropped += 1;
            return None;
        };
        out.clear();
        out.extend_from_slice(&self.collected[bundles.header() + bundles.known..]);
        Some(Unit::Burst(Burst {
            coding,
            frames,
            compact: true,
            tracked: true,
        }))
    }
}

fn best(votes: &[f32]) -> usize {
    votes
        .iter()
        .enumerate()
        .fold((0, f32::NEG_INFINITY), |best, (index, &vote)| {
            if vote > best.1 { (index, vote) } else { best }
        })
        .0
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "synth"))]
#[must_use]
pub fn wrap(payload: &[Complex<f32>], code: u8, pilots: bool, count: usize) -> Vec<Complex<f32>> {
    synth::Transmitter::new(synth::Codes::default()).legacy(payload, code, pilots, count)
}

#[cfg(test)]
mod tests;
