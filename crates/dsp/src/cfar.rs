/// How wide the noise estimate around a cell under test reaches, and how often a false alarm is
/// acceptable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CfarParams {
    pub guard_range: usize,
    pub guard_doppler: usize,
    pub train_range: usize,
    pub train_doppler: usize,
    pub probability_false_alarm: f32,
    /// Detections below this margin are dropped whatever the statistics say, so a surface that is
    /// all noise cannot produce a wall of marks.
    pub min_snr_db: f32,
    /// Doppler rows either side of zero that are never reported. Everything that is not moving
    /// lands there — the direct path, the ground, and whatever the reference antenna also hears —
    /// and reporting it would bury the targets in a ridge of clutter.
    pub zero_doppler_guard: usize,
}

impl Default for CfarParams {
    fn default() -> Self {
        Self {
            guard_range: 2,
            guard_doppler: 1,
            train_range: 8,
            train_doppler: 4,
            probability_false_alarm: 1e-4,
            min_snr_db: 6.0,
            zero_doppler_guard: 1,
        }
    }
}

impl CfarParams {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.train_range > 0
            && self.train_doppler > 0
            && (0.0..1.0).contains(&self.probability_false_alarm)
            && self.probability_false_alarm > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub range_bin: usize,
    pub doppler_bin: usize,
    pub snr_db: f32,
}

/// Merges detections that touch into one, keeping the strongest cell of each.
///
/// One target lights several neighbouring cells, and reporting each of them is reporting one
/// aircraft as four. Nothing here decides what a target is — it only refuses to count a single
/// bright patch more than once.
pub fn cluster(detections: &mut Vec<Detection>) {
    detections.sort_by(|a, b| b.snr_db.total_cmp(&a.snr_db));
    let mut kept: Vec<Detection> = Vec::with_capacity(detections.len());
    for detection in detections.iter() {
        let touching = kept.iter().any(|other| {
            other.range_bin.abs_diff(detection.range_bin) <= 1
                && other.doppler_bin.abs_diff(detection.doppler_bin) <= 1
        });
        if !touching {
            kept.push(*detection);
        }
    }
    *detections = kept;
}

/// Two-dimensional cell-averaging CFAR with a guard ring.
///
/// The threshold at each cell is set from its own neighbourhood rather than from a global figure,
/// which is what keeps a strong clutter ridge from swamping the whole surface with detections.
pub fn detect(
    surface: &[f32],
    ranges: usize,
    dopplers: usize,
    params: &CfarParams,
    out: &mut Vec<Detection>,
) {
    out.clear();
    if ranges == 0 || dopplers == 0 || surface.len() < ranges * dopplers || !params.valid() {
        return;
    }
    let cells = training_cells(params);
    if cells == 0 {
        return;
    }
    let alpha = cells as f32 * (params.probability_false_alarm.powf(-1.0 / cells as f32) - 1.0);
    let floor = 10f32.powf(params.min_snr_db / 10.0);
    let centre = (dopplers - 1) / 2;
    for doppler in 0..dopplers {
        if doppler.abs_diff(centre) <= params.zero_doppler_guard {
            continue;
        }
        for range in 0..ranges {
            let cell = surface[doppler * ranges + range];
            let Some(noise) = neighbourhood(surface, ranges, dopplers, range, doppler, params)
            else {
                continue;
            };
            if cell <= noise * alpha.max(floor) {
                continue;
            }
            out.push(Detection {
                range_bin: range,
                doppler_bin: doppler,
                snr_db: 10.0 * (cell / noise.max(f32::MIN_POSITIVE)).log10(),
            });
        }
    }
}

const fn training_cells(params: &CfarParams) -> usize {
    let outer = (2 * (params.guard_range + params.train_range) + 1)
        * (2 * (params.guard_doppler + params.train_doppler) + 1);
    let inner = (2 * params.guard_range + 1) * (2 * params.guard_doppler + 1);
    outer - inner
}

/// The average power of the training ring, or `None` when the cell sits too close to an edge for
/// the ring to be filled — reporting a detection from half a window is how an edge turns into a
/// permanent false target.
fn neighbourhood(
    surface: &[f32],
    ranges: usize,
    dopplers: usize,
    range: usize,
    doppler: usize,
    params: &CfarParams,
) -> Option<f32> {
    let reach_range = params.guard_range + params.train_range;
    let reach_doppler = params.guard_doppler + params.train_doppler;
    if range < reach_range
        || doppler < reach_doppler
        || range + reach_range >= ranges
        || doppler + reach_doppler >= dopplers
    {
        return None;
    }
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for row in (doppler - reach_doppler)..=(doppler + reach_doppler) {
        for col in (range - reach_range)..=(range + reach_range) {
            let inside_guard = row.abs_diff(doppler) <= params.guard_doppler
                && col.abs_diff(range) <= params.guard_range;
            if inside_guard {
                continue;
            }
            sum += f64::from(surface[row * ranges + col]);
            count += 1;
        }
    }
    (count > 0).then(|| (sum / count as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy_surface(ranges: usize, dopplers: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        (0..ranges * dopplers)
            .map(|_| {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;
                let unit =
                    (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 24) as f32;
                0.5 + unit
            })
            .collect()
    }

    #[test]
    fn a_target_planted_in_noise_is_found_where_it_was_planted() {
        let (ranges, dopplers) = (128, 33);
        let mut surface = noisy_surface(ranges, dopplers, 0x5EED);
        surface[22 * ranges + 40] = 60.0;
        let mut detections = Vec::new();
        detect(
            &surface,
            ranges,
            dopplers,
            &CfarParams::default(),
            &mut detections,
        );
        assert!(
            detections
                .iter()
                .any(|d| d.range_bin == 40 && d.doppler_bin == 22 && d.snr_db > 12.0),
            "{detections:?}"
        );
    }

    #[test]
    fn noise_alone_stays_under_the_false_alarm_rate_it_was_given() {
        let (ranges, dopplers) = (256, 64);
        let surface = noisy_surface(ranges, dopplers, 0xBEEF);
        let mut detections = Vec::new();
        detect(
            &surface,
            ranges,
            dopplers,
            &CfarParams::default(),
            &mut detections,
        );
        let rate = detections.len() as f32 / (ranges * dopplers) as f32;
        assert!(rate < 0.001, "{} detections out of noise", detections.len());
    }

    #[test]
    fn a_cell_without_a_full_training_ring_is_left_alone() {
        let (ranges, dopplers) = (64, 16);
        let mut surface = vec![1.0f32; ranges * dopplers];
        surface[0] = 1e6;
        surface[dopplers - 1] = 1e6;
        let mut detections = Vec::new();
        detect(
            &surface,
            ranges,
            dopplers,
            &CfarParams::default(),
            &mut detections,
        );
        assert!(detections.is_empty(), "{detections:?}");
    }

    #[test]
    fn everything_standing_still_is_left_out_of_the_answer() {
        let (ranges, dopplers) = (128, 33);
        let mut surface = noisy_surface(ranges, dopplers, 0x0FF1);
        let centre = (dopplers - 1) / 2;
        for range in 0..ranges {
            surface[centre * ranges + range] = 200.0;
        }
        let mut detections = Vec::new();
        detect(
            &surface,
            ranges,
            dopplers,
            &CfarParams::default(),
            &mut detections,
        );
        assert!(
            detections.iter().all(|d| d.doppler_bin != centre),
            "a stationary ridge was reported: {detections:?}"
        );
    }

    #[test]
    fn parameters_that_describe_no_training_ring_are_refused() {
        let params = CfarParams {
            train_range: 0,
            ..CfarParams::default()
        };
        assert!(!params.valid());
        let mut detections = vec![Detection {
            range_bin: 1,
            doppler_bin: 1,
            snr_db: 1.0,
        }];
        detect(&[1.0; 16], 4, 4, &params, &mut detections);
        assert!(detections.is_empty());
    }

    #[test]
    fn one_bright_patch_is_one_detection() {
        let mut detections = vec![
            Detection {
                range_bin: 40,
                doppler_bin: 6,
                snr_db: 9.0,
            },
            Detection {
                range_bin: 41,
                doppler_bin: 6,
                snr_db: 12.0,
            },
            Detection {
                range_bin: 41,
                doppler_bin: 7,
                snr_db: 8.0,
            },
            Detection {
                range_bin: 90,
                doppler_bin: 20,
                snr_db: 7.0,
            },
        ];
        cluster(&mut detections);
        assert_eq!(detections.len(), 2);
        assert_eq!(
            detections[0].range_bin, 41,
            "the strongest cell speaks for the patch"
        );
        assert_eq!(detections[0].snr_db, 12.0);
        assert_eq!(detections[1].range_bin, 90);
    }
}
