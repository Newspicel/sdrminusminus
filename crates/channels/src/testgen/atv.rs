//! Reference ATV transmitter: a standards-timed analog raster, modulated the way the band it
//! belongs to modulates it.
use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_wire::{AtvColor, AtvModulation, AtvParams, AtvStandard};

/// Video levels, sync tip to peak white, as the standards define them: blanking sits 30 % of
/// the way up and the picture occupies everything above it.
const SYNC: f32 = 0.0;
const BLANKING: f32 = 0.3;

/// Peak-carrier fraction the picture is keyed down to at peak white. 87.5 % leaves white at
/// 12.5 % of the sync tip, which is what broadcast negative modulation transmits.
const AM_DEPTH: f64 = 0.875;

/// Peak deviation of the FM form, in Hz. Small next to real FM ATV on purpose: Carson's rule
/// has to keep the transmission inside the channel the test then filters it through.
const FM_DEVIATION_HZ: f64 = 150_000.0;
const SOUND_DEVIATION_HZ: f64 = 50_000.0;
const SOUND_LEVEL: f64 = 0.12;
const EMBEDDED_SOUND_LEVEL: f64 = 0.01;
const PAL_SUBCARRIER_HZ: f64 = 4_433_618.75;
const NTSC_SUBCARRIER_HZ: f64 = 3_579_545.0;

/// Luma of the vertical bars [`bars`] transmits, black to white.
pub const BAR_LEVELS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
pub const COLOR_BARS: [[f32; 3]; 8] = [
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [0.0, 1.0, 0.0],
    [1.0, 0.0, 1.0],
    [1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, 0.0],
];

/// One standard's line, in seconds, and the vertical structure of one of its fields.
#[derive(Clone, Copy, Debug)]
struct Layout {
    line_s: f64,
    front_porch_s: f64,
    sync_s: f64,
    back_porch_s: f64,
    active_s: f64,
    /// Half-lines of equalizing pulses before the broad group, of broad pulses, and of
    /// equalizing pulses after it.
    pre_eq: u16,
    broad: u16,
    post_eq: u16,
    /// Half-lines in one field: half the frame's lines, counted this way because the vertical
    /// structure is on a half-line grid and the frame's is not.
    field_half_lines: u16,
    /// Whole lines of blanking between the vertical group and the first line of picture.
    blank_after_group: u16,
    /// Lines of picture in one field.
    active_lines: u16,
}

fn layout(standard: AtvStandard) -> Layout {
    let (sync_us, back_us, active_us, pre_eq, broad, post_eq, blank_after_group, active_lines) =
        match standard {
            AtvStandard::Ccir625 => (4.7, 5.8, 51.95, 5, 5, 5, 15, 288),
            AtvStandard::Eia525 => (4.7, 4.5, 52.6, 6, 6, 6, 11, 240),
            AtvStandard::SystemA405 => (9.0, 5.8, 82.2, 0, 8, 0, 10, 188),
        };
    // The line period comes from the standard's line rate, not from a rounded figure: a frame
    // that is a few samples long closes on the wrong sample and drifts a receiver every frame.
    let line_s = 1.0 / standard.line_rate_hz();
    Layout {
        line_s,
        front_porch_s: line_s - (sync_us + back_us + active_us) * 1e-6,
        sync_s: sync_us * 1e-6,
        back_porch_s: back_us * 1e-6,
        active_s: active_us * 1e-6,
        pre_eq,
        broad,
        post_eq,
        field_half_lines: standard.lines(),
        blank_after_group,
        active_lines,
    }
}

/// What a segment of the raster carries. Every one is either a half-line of the vertical
/// structure or a whole line.
#[derive(Clone, Copy)]
enum Seg {
    /// A half-line whose first half-sync marks time through the vertical interval.
    Equalizing,
    /// A half-line of the broad group: sync for all of it but the last sync width.
    Broad,
    /// A half-line of plain blanking, which is what carries the interlace offset.
    HalfBlank,
    /// A whole line, with picture in its active window or blanking instead.
    Line { picture: bool },
}

/// A transmitter for one set of ATV settings at one sample rate.
pub struct AtvSource {
    rate: f64,
    layout: Layout,
    modulation: AtvModulation,
    invert: bool,
    interlace: bool,
    color: AtvColor,
    sound_subcarrier_hz: Option<f64>,
}

impl AtvSource {
    #[must_use]
    pub fn new(params: &AtvParams, rate: f64) -> Self {
        Self {
            rate,
            layout: layout(params.standard),
            modulation: params.modulation,
            invert: params.invert,
            interlace: params.interlace,
            color: params.color,
            sound_subcarrier_hz: params.sound_subcarrier_hz,
        }
    }

    /// The frame's segments. `second` selects the field whose vertical group arrives half a
    /// line late — the only difference between the two, and the whole of interlace.
    fn field(&self, second: bool) -> Vec<Seg> {
        let l = self.layout;
        let mut segs = Vec::new();
        // The half-line that displaces this field's line grid. Emitted before the vertical
        // group so the group itself, and with it the receiver's field-parity test, moves with it.
        if second {
            segs.push(Seg::HalfBlank);
        }
        segs.extend(std::iter::repeat_n(Seg::Equalizing, usize::from(l.pre_eq)));
        segs.extend(std::iter::repeat_n(Seg::Broad, usize::from(l.broad)));
        segs.extend(std::iter::repeat_n(Seg::Equalizing, usize::from(l.post_eq)));
        let group = usize::from(second) + usize::from(l.pre_eq + l.broad + l.post_eq);
        let remaining = usize::from(l.field_half_lines) - group;
        let lines = remaining / 2;
        for k in 0..lines {
            let first = usize::from(l.blank_after_group);
            segs.push(Seg::Line {
                picture: k >= first && k < first + usize::from(l.active_lines),
            });
        }
        if remaining % 2 == 1 {
            segs.push(Seg::HalfBlank);
        }
        segs
    }

    /// Level of one segment at `u` seconds into it, `luma` sampled across the active window.
    fn level(&self, seg: Seg, u: f64, line: u64, pixel: &dyn Fn(f64) -> [f32; 3]) -> f32 {
        let l = self.layout;
        let half = l.line_s / 2.0;
        match seg {
            Seg::Equalizing => {
                if u < l.sync_s / 2.0 {
                    SYNC
                } else {
                    BLANKING
                }
            }
            Seg::Broad => {
                if u < half - l.sync_s {
                    SYNC
                } else {
                    BLANKING
                }
            }
            Seg::HalfBlank => BLANKING,
            Seg::Line { picture } => {
                let sync_end = l.front_porch_s + l.sync_s;
                let active_start = sync_end + l.back_porch_s;
                if u < l.front_porch_s || u >= active_start {
                    if u >= active_start && picture {
                        let x = (u - active_start) / l.active_s;
                        let [r, g, b] = pixel(x.clamp(0.0, 1.0));
                        let y = 0.299 * r + 0.587 * g + 0.114 * b;
                        let mut composite = BLANKING + (1.0 - BLANKING) * y;
                        if let Some(carrier) = color_carrier(self.color) {
                            let chroma_u = 0.492 * (b - y);
                            let mut chroma_v = 0.877 * (r - y);
                            if self.color == AtvColor::Pal && line % 2 == 1 {
                                chroma_v = -chroma_v;
                            }
                            let phase = TAU * carrier * (u - l.front_porch_s);
                            composite += 0.35
                                * (chroma_u * phase.cos() as f32 + chroma_v * phase.sin() as f32);
                        }
                        return composite;
                    }
                    BLANKING
                } else if u < sync_end {
                    SYNC
                } else {
                    let since_sync = u - l.front_porch_s;
                    if let Some(carrier) = color_carrier(self.color)
                        && (5.5e-6..8.0e-6).contains(&since_sync)
                    {
                        BLANKING + 0.12 * (TAU * carrier * since_sync).cos() as f32
                    } else {
                        BLANKING
                    }
                }
            }
        }
    }

    fn seg_seconds(&self, seg: Seg) -> f64 {
        match seg {
            Seg::Line { .. } => self.layout.line_s,
            _ => self.layout.line_s / 2.0,
        }
    }

    /// Render `frames` frames of `luma` as the video waveform, sync tip 0.0 to peak white 1.0.
    fn video(&self, frames: u32, luma: &dyn Fn(f64) -> f32) -> Vec<f32> {
        self.composite(
            frames,
            &|x| {
                let y = luma(x);
                [y, y, y]
            },
            None,
        )
    }

    fn composite(
        &self,
        frames: u32,
        pixel: &dyn Fn(f64) -> [f32; 3],
        sound_hz: Option<f64>,
    ) -> Vec<f32> {
        let mut segs = Vec::new();
        for _ in 0..frames {
            segs.extend(self.field(false));
            // A progressive source repeats the same field structure: no half-line displacement,
            // so a receiver weaves nothing and every vertical sync opens a whole picture.
            segs.extend(self.field(self.interlace));
        }
        let mut out = Vec::new();
        // Boundaries are tracked in fractional samples and rounded once, so a half-line offset
        // survives a rate that does not divide the line period.
        let mut start = 0.0f64;
        let mut line = 0u64;
        for seg in segs {
            let seconds = self.seg_seconds(seg);
            let end = start + seconds * self.rate;
            let first = start.round() as usize;
            let count = end.round() as usize - first;
            for k in 0..count {
                let u = (first + k) as f64 - start;
                out.push(self.level(seg, u / self.rate, line, pixel));
            }
            if matches!(seg, Seg::Line { .. }) {
                line += 1;
            }
            start = end;
        }
        if self.modulation == AtvModulation::Fm {
            add_sound_subcarrier(&mut out, self.rate, self.sound_subcarrier_hz.zip(sound_hz));
        }
        out
    }

    /// Modulate a video waveform onto complex baseband.
    fn modulate(&self, video: &[f32], sound_hz: Option<f64>) -> Vec<Complex<f32>> {
        let mut phase = 0.0f64;
        let mut sound_phase = 0.0f64;
        video
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let m = f64::from(if self.invert { 1.0 - v } else { v });
                match self.modulation {
                    AtvModulation::Am => {
                        let mut sample = Complex::new((1.0 - AM_DEPTH * m) as f32, 0.0);
                        if let Some((carrier, tone)) = self.sound_subcarrier_hz.zip(sound_hz) {
                            let audio = (TAU * tone * i as f64 / self.rate).sin();
                            sound_phase += TAU * (carrier + SOUND_DEVIATION_HZ * audio) / self.rate;
                            sample += Complex::from_polar(SOUND_LEVEL as f32, sound_phase as f32);
                        }
                        sample
                    }
                    AtvModulation::Fm => {
                        phase += TAU * FM_DEVIATION_HZ * (2.0 * m - 1.0) / self.rate;
                        Complex::from_polar(1.0, phase as f32)
                    }
                }
            })
            .collect()
    }
}

/// `frames` frames of vertical bars at [`BAR_LEVELS`], as complex baseband IQ.
#[must_use]
pub fn bars(source: &AtvSource, frames: u32) -> Vec<Complex<f32>> {
    let video = source.video(frames, &|x| {
        let bar = ((x * BAR_LEVELS.len() as f64) as usize).min(BAR_LEVELS.len() - 1);
        BAR_LEVELS[bar]
    });
    source.modulate(&video, None)
}

/// Eight standard RGB bars and a 1 kHz tone on the configured sound carrier.
#[must_use]
pub fn color_bars_with_tone(source: &AtvSource, frames: u32) -> Vec<Complex<f32>> {
    let video = source.composite(
        frames,
        &|x| {
            let bar = ((x * COLOR_BARS.len() as f64) as usize).min(COLOR_BARS.len() - 1);
            COLOR_BARS[bar]
        },
        Some(1_000.0),
    );
    source.modulate(&video, Some(1_000.0))
}

fn color_carrier(color: AtvColor) -> Option<f64> {
    match color {
        AtvColor::Monochrome => None,
        AtvColor::Pal => Some(PAL_SUBCARRIER_HZ),
        AtvColor::Ntsc => Some(NTSC_SUBCARRIER_HZ),
    }
}

fn add_sound_subcarrier(video: &mut [f32], rate: f64, sound: Option<(f64, f64)>) {
    let Some((carrier, tone)) = sound else {
        return;
    };
    let mut phase = 0.0;
    for (i, sample) in video.iter_mut().enumerate() {
        let audio = (TAU * tone * i as f64 / rate).sin();
        phase += TAU * (carrier + SOUND_DEVIATION_HZ * audio) / rate;
        *sample += EMBEDDED_SOUND_LEVEL as f32 * phase.cos() as f32;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::AtvParams;

    use super::*;

    const RATE: f64 = 2_000_000.0;

    fn source(standard: AtvStandard) -> AtvSource {
        AtvSource::new(
            &AtvParams {
                standard,
                ..AtvParams::default()
            },
            RATE,
        )
    }

    /// A frame must be exactly the standard's own duration: the vertical structure is laid out
    /// in half-lines and the picture in whole ones, and a raster that does not close on itself
    /// would drift a receiver by a line a frame however good its flywheel is.
    #[test]
    fn a_frame_lasts_exactly_one_frame_period() {
        for standard in [
            AtvStandard::Ccir625,
            AtvStandard::Eia525,
            AtvStandard::SystemA405,
        ] {
            let src = source(standard);
            let video = src.video(2, &|_| 1.0);
            let want = 2.0 * RATE / standard.frame_rate_hz();
            let got = video.len() as f64;
            assert!(
                (got - want).abs() <= 2.0,
                "{standard:?}: {got} samples for two frames, expected {want}"
            );
        }
    }

    /// The two fields must differ by exactly one half-line, since that displacement is the only
    /// thing a receiver can tell them apart by.
    #[test]
    fn the_second_field_is_displaced_by_half_a_line() {
        let src = source(AtvStandard::Ccir625);
        let first: f64 = src
            .field(false)
            .into_iter()
            .map(|s| src.seg_seconds(s))
            .sum();
        let second: f64 = src
            .field(true)
            .into_iter()
            .map(|s| src.seg_seconds(s))
            .sum();
        assert!((first - second).abs() < 1e-12, "{first} vs {second}");
        let group = |second: bool| {
            src.field(second)
                .into_iter()
                .take_while(|s| !matches!(s, Seg::Broad))
                .map(|s| src.seg_seconds(s))
                .sum::<f64>()
        };
        let offset = group(true) - group(false);
        assert!(
            (offset - src.layout.line_s / 2.0).abs() < 1e-12,
            "broad group moved by {offset} s, expected half a line"
        );
    }

    /// Sync must be the lowest level in the waveform and white the highest, with blanking where
    /// the standards put it — the levels every receiver slices against.
    #[test]
    fn levels_sit_where_the_standards_put_them() {
        let src = source(AtvStandard::Ccir625);
        let video = src.video(1, &|_| 1.0);
        let lo = video.iter().copied().fold(f32::MAX, f32::min);
        let hi = video.iter().copied().fold(f32::MIN, f32::max);
        assert_eq!(lo, SYNC);
        assert_eq!(hi, 1.0);
        let l = src.layout;
        let at = |seconds: f64| video[(seconds * RATE) as usize];
        let group = f64::from(l.pre_eq + l.broad + l.post_eq) * l.line_s / 2.0;
        let line_start = group + 20.0 * l.line_s;
        assert_eq!(
            at(line_start + l.front_porch_s + l.sync_s + l.back_porch_s / 2.0),
            BLANKING
        );
        assert_eq!(at(line_start + l.front_porch_s + l.sync_s / 2.0), SYNC);
    }

    /// Negative modulation is the claim the AM form makes: peak carrier at the sync tip, and
    /// white keyed down to a small fraction of it.
    #[test]
    fn am_keys_sync_to_peak_carrier() {
        let src = source(AtvStandard::Ccir625);
        let iq = bars(&src, 1);
        let peak = iq.iter().map(|s| s.norm()).fold(f32::MIN, f32::max);
        let trough = iq.iter().map(|s| s.norm()).fold(f32::MAX, f32::min);
        assert!((peak - 1.0).abs() < 1e-6, "peak {peak}");
        assert!(
            (trough - (1.0 - AM_DEPTH as f32)).abs() < 1e-6,
            "trough {trough}"
        );
    }
}
