#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumView {
    pub start: f64,
    pub end: f64,
}

pub const FULL_VIEW: SpectrumView = SpectrumView {
    start: 0.0,
    end: 1.0,
};

const MIN_WIDTH: f64 = 1.0 / 512.0;
const WHEEL_ZOOM: f64 = 1.2;
pub const MARKER_LABEL_GAP: f64 = 0.18;
const LABEL_CHAR_PX: f64 = 6.0;
const LABEL_CHROME_PX: f64 = 16.0;

#[must_use]
pub fn above(value: f64, floor: f64) -> bool {
    value > floor
}

#[must_use]
pub fn at_least(value: f64, floor: f64) -> bool {
    value >= floor
}

impl Default for SpectrumView {
    fn default() -> Self {
        FULL_VIEW
    }
}

impl SpectrumView {
    #[must_use]
    pub fn width(self) -> f64 {
        self.end - self.start
    }

    #[must_use]
    pub fn is_full(self) -> bool {
        self.start <= 0.0 && self.end >= 1.0
    }

    #[must_use]
    pub fn zoom(self, at: f64, factor: f64) -> Self {
        let at = clamp01(at);
        let anchor = self.start + at * self.width();
        let next = (self.width() / factor).clamp(MIN_WIDTH, 1.0);
        slide(anchor - at * next, next)
    }

    #[must_use]
    pub fn pan(self, by_screen_fraction: f64) -> Self {
        slide(self.start + by_screen_fraction * self.width(), self.width())
    }

    #[must_use]
    pub fn wheel(self, delta_x: f64, delta_y: f64, at: f64, width_px: f64) -> Self {
        if delta_y.abs() >= delta_x.abs() {
            let factor = if delta_y < 0.0 {
                WHEEL_ZOOM
            } else {
                1.0 / WHEEL_ZOOM
            };
            return self.zoom(at, factor);
        }
        self.pan(delta_x / width_px.max(1.0))
    }

    #[must_use]
    pub fn to_span(self, at: f64) -> f64 {
        self.start + at * self.width()
    }

    #[must_use]
    pub fn place(self, fraction: f64) -> f64 {
        (fraction - self.start) / self.width()
    }
}

#[must_use]
pub fn offset_to_span(offset_hz: f64, span_hz: f64) -> f64 {
    0.5 + offset_hz / span_hz
}

#[must_use]
pub fn span_to_offset(fraction: f64, span_hz: f64) -> f64 {
    (fraction - 0.5) * span_hz
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub hz: f64,
    pub at: f64,
}

#[must_use]
pub fn frequency_ticks(centre_hz: f64, span_hz: f64, view: SpectrumView, target: f64) -> Vec<Tick> {
    let visible = span_hz * view.width();
    if !above(visible, 0.0) || !at_least(target, 1.0) {
        return Vec::new();
    }
    let low_hz = centre_hz + span_to_offset(view.start, span_hz);
    let step = nice_step(visible / target);
    let mut ticks = Vec::new();
    let mut hz = (low_hz / step).ceil() * step;
    while hz <= low_hz + visible {
        ticks.push(Tick {
            hz,
            at: (hz - low_hz) / visible,
        });
        hz += step;
    }
    ticks
}

#[must_use]
pub fn decibel_ticks(db_min: f64, db_max: f64, target: f64) -> Vec<f64> {
    if !above(db_max, db_min) || !at_least(target, 1.0) {
        return Vec::new();
    }
    let step = nice_step((db_max - db_min) / target);
    let mut ticks = Vec::new();
    let mut db = (db_min / step).ceil() * step;
    while db <= db_max {
        ticks.push(db);
        db += step;
    }
    ticks
}

#[must_use]
pub fn nice_step(raw: f64) -> f64 {
    if !above(raw, 0.0) {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let nice = if normalized < 1.5 {
        1.0
    } else if normalized < 3.0 {
        2.0
    } else if normalized < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

#[must_use]
pub fn label_width(text: &str, plot_width_px: f64) -> f64 {
    if plot_width_px > 0.0 {
        (text.chars().count() as f64 * LABEL_CHAR_PX + LABEL_CHROME_PX) / plot_width_px
    } else {
        MARKER_LABEL_GAP
    }
}

pub trait Placed {
    fn at(&self) -> f64;
    fn width(&self) -> f64;
}

#[must_use]
pub fn cluster_markers<T: Placed + Clone>(markers: &[T]) -> Vec<Vec<T>> {
    let mut sorted = markers.to_vec();
    sorted.sort_by(|a, b| a.at().total_cmp(&b.at()));
    let mut clusters: Vec<Vec<T>> = Vec::new();
    for marker in sorted {
        let joins = clusters
            .last()
            .and_then(|open| open.first())
            .is_some_and(|anchor| {
                marker.at() - anchor.at() < f64::midpoint(anchor.width(), marker.width())
            });
        match clusters.last_mut() {
            Some(open) if joins => open.push(marker),
            _ => clusters.push(vec![marker]),
        }
    }
    clusters
}

fn slide(start: f64, width: f64) -> SpectrumView {
    let width = width.min(1.0);
    let start = start.max(0.0).min(1.0 - width);
    SpectrumView {
        start,
        end: start + width,
    }
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn zooming_holds_the_frequency_under_the_cursor_still() {
        let before = FULL_VIEW.to_span(0.25);
        assert!(close(FULL_VIEW.zoom(0.25, 4.0).to_span(0.25), before));
        let mut view = FULL_VIEW;
        let target = view.to_span(0.7);
        for _ in 0..8 {
            view = view.zoom(0.7, 1.2);
        }
        assert!(close(view.to_span(0.7), target));
    }

    #[test]
    fn zooming_never_leaves_the_span() {
        let edge = FULL_VIEW.zoom(0.0, 8.0);
        assert_eq!(edge.start, 0.0);
        assert!(close(edge.width(), 0.125));
        assert_eq!(FULL_VIEW.zoom(1.0, 8.0).end, 1.0);
    }

    #[test]
    fn zooming_clamps_at_full_span_and_at_the_floor() {
        assert!(FULL_VIEW.zoom(0.5, 0.25).is_full());
        let mut view = FULL_VIEW;
        for _ in 0..100 {
            view = view.zoom(0.5, 2.0);
        }
        assert!(close(view.width(), 1.0 / 512.0));
    }

    #[test]
    fn panning_moves_by_the_pointer_distance_and_stops_at_the_edge() {
        let view = FULL_VIEW.zoom(0.5, 4.0);
        assert!(close(view.pan(0.5).start - view.start, 0.125));
        let stopped = view.pan(-5.0);
        assert_eq!(stopped.start, 0.0);
        assert!(close(stopped.width(), view.width()));
    }

    #[test]
    fn the_wheel_zooms_vertically_and_pans_sideways() {
        let closer = FULL_VIEW.wheel(0.0, -100.0, 0.5, 400.0);
        assert!(close(closer.width(), 1.0 / 1.2));
        assert!(closer.wheel(0.0, 100.0, 0.5, 400.0).is_full());
        let zoomed = FULL_VIEW.zoom(0.5, 4.0);
        let panned = zoomed.wheel(100.0, 2.0, 0.5, 400.0);
        assert!(close(panned.start - zoomed.start, 0.25 * 0.25));
        assert_eq!(FULL_VIEW.wheel(100.0, 0.0, 0.5, 400.0), FULL_VIEW);
        assert!(close(
            FULL_VIEW.wheel(-50.0, -50.0, 0.5, 400.0).width(),
            1.0 / 1.2
        ));
    }

    #[test]
    fn view_and_span_map_both_ways_without_clamping() {
        let view = FULL_VIEW.zoom(0.3, 6.0);
        assert!(close(view.place(view.to_span(0.42)), 0.42));
        let window = SpectrumView {
            start: 0.4,
            end: 0.6,
        };
        assert!(window.place(0.1) < 0.0);
        assert!(window.place(0.9) > 1.0);
        assert_eq!(offset_to_span(0.0, 2_048_000.0), 0.5);
        assert_eq!(offset_to_span(512_000.0, 2_048_000.0), 0.75);
    }

    #[test]
    fn nice_steps_walk_the_one_two_five_ladder() {
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(1.4), 1.0);
        assert_eq!(nice_step(2.9), 2.0);
        assert_eq!(nice_step(6.0), 5.0);
        assert_eq!(nice_step(9.0), 10.0);
        assert_eq!(nice_step(230_000.0), 200_000.0);
        assert_eq!(nice_step(0.0), 1.0);
        assert_eq!(nice_step(f64::NAN), 1.0);
    }

    #[test]
    fn frequency_ticks_land_on_round_values_and_refine_with_zoom() {
        let wide = frequency_ticks(100e6, 2.048e6, FULL_VIEW, 6.0);
        assert!(wide.len() > 3);
        for tick in &wide {
            assert_eq!(tick.hz % 500_000.0, 0.0);
            assert!((0.0..=1.0).contains(&tick.at));
        }
        let near = frequency_ticks(100e6, 2.048e6, FULL_VIEW.zoom(0.5, 16.0), 6.0);
        assert!(near[1].hz - near[0].hz < wide[1].hz - wide[0].hz);
        assert!(frequency_ticks(100e6, 0.0, FULL_VIEW, 6.0).is_empty());
    }

    #[test]
    fn decibel_ticks_cover_the_range_on_round_values() {
        let ticks = decibel_ticks(-91.0, -11.0, 4.0);
        assert!(ticks.iter().all(|db| db % 20.0 == 0.0));
        assert!(ticks[0] >= -91.0);
        assert!(ticks[ticks.len() - 1] <= -11.0);
        assert!(decibel_ticks(0.0, -10.0, 4.0).is_empty());
    }

    #[test]
    fn a_label_takes_a_smaller_share_of_a_wider_plot() {
        assert!(label_width("NFM +0 kHz", 400.0) > label_width("NFM +0 kHz", 1600.0));
        assert_eq!(label_width("NFM +0 kHz", 0.0), MARKER_LABEL_GAP);
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Mark {
        id: u32,
        at: f64,
        width: f64,
    }

    impl Placed for Mark {
        fn at(&self) -> f64 {
            self.at
        }
        fn width(&self) -> f64 {
            self.width
        }
    }

    fn marks(at: &[f64], width: f64) -> Vec<Mark> {
        at.iter()
            .enumerate()
            .map(|(id, at)| Mark {
                id: id as u32,
                at: *at,
                width,
            })
            .collect()
    }

    fn sizes(clusters: &[Vec<Mark>]) -> Vec<usize> {
        clusters.iter().map(Vec::len).collect()
    }

    #[test]
    fn markers_cluster_only_where_their_captions_collide() {
        assert_eq!(
            sizes(&cluster_markers(&marks(&[0.1, 0.5, 0.9], 0.18))),
            [1, 1, 1]
        );
        assert_eq!(sizes(&cluster_markers(&marks(&[0.4, 0.4, 0.4], 0.18))), [3]);
        assert_eq!(
            sizes(&cluster_markers(&marks(&[0.0, 0.1, 0.2, 0.3], 0.18))),
            [2, 2]
        );
        assert_eq!(cluster_markers(&marks(&[0.4, 0.5], 0.18)).len(), 1);
        assert_eq!(cluster_markers(&marks(&[0.4, 0.5], 0.06)).len(), 2);
        let mixed = vec![
            Mark {
                id: 0,
                at: 0.4,
                width: 0.02,
            },
            Mark {
                id: 1,
                at: 0.5,
                width: 0.3,
            },
        ];
        assert_eq!(cluster_markers(&mixed).len(), 1);
    }

    #[test]
    fn clusters_are_ordered_by_position_not_arrival() {
        let placed = vec![
            Mark {
                id: 9,
                at: 0.42,
                width: 0.18,
            },
            Mark {
                id: 4,
                at: 0.4,
                width: 0.18,
            },
            Mark {
                id: 1,
                at: 0.9,
                width: 0.18,
            },
        ];
        let ids: Vec<Vec<u32>> = cluster_markers(&placed)
            .iter()
            .map(|group| group.iter().map(|mark| mark.id).collect())
            .collect();
        assert_eq!(ids, vec![vec![4, 9], vec![1]]);
    }
}
