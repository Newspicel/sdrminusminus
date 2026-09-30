use sdrmm_wire::{BeamMode, BeamformerReading, LaneWeight, ProcessorReading};

use super::{Beamformer, Path, steered};
use crate::array_processor::{ProcessorOutput, stamp_at};

pub const PATTERN_SPAN_DB: f32 = 40.0;
pub const WEIGHT_FLOOR_DB: f32 = -100.0;
pub const POWER_FLOOR_DB: f32 = -150.0;

fn db(power: f32) -> f32 {
    if power > 0.0 {
        (10.0 * power.log10()).max(POWER_FLOOR_DB)
    } else {
        POWER_FLOOR_DB
    }
}

fn pattern_byte(level: f32) -> u8 {
    let db = if level > 0.0 {
        10.0 * level.log10()
    } else {
        -PATTERN_SPAN_DB
    };
    (255.0 * (1.0 + db / PATTERN_SPAN_DB))
        .round()
        .clamp(0.0, 255.0) as u8
}

impl Beamformer {
    pub(super) fn report(&mut self, unix_ns: u64, out: &mut ProcessorOutput<'_>) {
        let Some(slot) = out.report() else {
            return;
        };
        if !matches!(slot, ProcessorReading::Beamformer(_)) {
            *slot = ProcessorReading::Beamformer(BeamformerReading::reserved());
        }
        if let ProcessorReading::Beamformer(reading) = slot {
            self.fill(reading, unix_ns);
        }
        out.publish_report();
        self.flags.diverged = false;
    }

    fn fill(&mut self, reading: &mut BeamformerReading, unix_ns: u64) {
        let path = self.path();
        stamp_at(&mut reading.at, unix_ns);
        reading.mode = self.settings.mode;
        reading.sinr_gain_db = self.metrics.gain_db;
        reading.snr_db = self.metrics.snr_db;
        reading.output_db = db(if path == Path::Tdl {
            self.beam_power
        } else {
            self.metrics.output_power
        });
        let steering = steered(self.settings.mode);
        reading.steer_deg = self
            .resolved
            .steer
            .filter(|_| steering)
            .map(|direction| direction.azimuth_deg as f32);
        reading.steer_age_ms = self.resolved.age_ms.filter(|_| steering);
        reading.cancelled_db = match (self.settings.mode, path) {
            (BeamMode::Canceller, Path::Tdl) => self.tdl.as_ref().map(|tdl| tdl.suppression_db()),
            (BeamMode::Canceller, _) => self.metrics.cancelled_db,
            _ => None,
        };
        reading.loading_used = self.loading_used;
        reading.resets = self.adaptive_resets();
        reading.out_center_hz = self.format.center_hz;
        reading.out_rate = self.format.sample_rate;
        reading.no_steer = self.flags.no_steer;
        reading.steer_stale = self.flags.steer_stale;
        reading.singular = self.flags.singular;
        reading.diverged = self.flags.diverged;
        reading.band_full = self.flags.band_full;
        self.fill_lists(reading, path);
    }

    fn fill_lists(&mut self, reading: &mut BeamformerReading, path: Path) {
        reading.nulls_deg.clear();
        for &null in &self.resolved.placed[..self.resolved.placed_count] {
            self.faults.push_capped(&mut reading.nulls_deg, null as f32);
        }
        reading.null_depths_db.clear();
        let depths = self.metrics.nulls.min(self.metrics.null_depth_db.len());
        for &depth in &self.metrics.null_depth_db[..depths] {
            self.faults.push_capped(&mut reading.null_depths_db, depth);
        }
        reading.weights.clear();
        if path != Path::Tdl {
            let weights = self.effective.as_slice();
            let largest = weights.iter().map(|w| w.norm()).fold(0.0f32, f32::max);
            for weight in weights {
                let ratio = if largest > 0.0 {
                    weight.norm() / largest
                } else {
                    0.0
                };
                let amplitude_db = if ratio > 0.0 {
                    (20.0 * ratio.log10()).max(WEIGHT_FLOOR_DB)
                } else {
                    WEIGHT_FLOOR_DB
                };
                let entry = LaneWeight {
                    amplitude_db,
                    phase_deg: weight.arg().to_degrees(),
                };
                self.faults.push_capped(&mut reading.weights, entry);
            }
        }
        reading.pattern.clear();
        if self.has_pattern {
            for &level in &self.pattern {
                self.faults
                    .push_capped(&mut reading.pattern, pattern_byte(level));
            }
        }
    }

    fn adaptive_resets(&self) -> u32 {
        let tdl = self.tdl.as_ref().map_or(0, |tdl| tdl.resets());
        self.gsc
            .resets()
            .saturating_add(self.cma.resets())
            .saturating_add(tdl)
    }
}
