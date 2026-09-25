use super::view::above;
use std::{cell::RefCell, collections::HashMap};

use sdrmm_wire::{bandplan::BandPlan, channel::ChannelInfo, channel::ChannelParams};

use super::{
    bands::{identify, suggested_at},
    view::{SpectrumView, span_to_offset},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopePick {
    pub hz: f64,
    pub offset_hz: f64,
}

#[must_use]
pub fn stream_channels(channels: &[ChannelInfo], stream: u32) -> Vec<ChannelInfo> {
    channels
        .iter()
        .filter(|channel| channel.stream == stream)
        .cloned()
        .collect()
}

#[must_use]
pub fn pick_at(centre_hz: f64, span_hz: f64, view: SpectrumView, at: f64) -> ScopePick {
    let offset_hz = span_to_offset(view.to_span(at), span_hz).round();
    ScopePick {
        hz: centre_hz + offset_hz,
        offset_hz,
    }
}

#[must_use]
pub fn drag_tune_hz(
    centre_hz: f64,
    span_hz: f64,
    view: SpectrumView,
    delta_px: f64,
    width_px: f64,
) -> f64 {
    if !above(span_hz, 0.0) || !above(width_px, 0.0) {
        return centre_hz.round();
    }
    (centre_hz + delta_px / width_px * span_hz * view.width()).round()
}

#[must_use]
pub fn pick_text(pick: ScopePick) -> (String, String) {
    let offset = pick.offset_hz.round();
    let sign = if offset < 0.0 { "-" } else { "+" };
    (
        format!("{} Hz", pick.hz.round()),
        format!("{sign}{} Hz", offset.abs()),
    )
}

#[must_use]
pub fn format_mhz(hz: f64) -> String {
    format!("{:.4} MHz", hz / 1e6)
}

#[must_use]
pub fn format_hz(hz: f64) -> String {
    if !hz.is_finite() {
        return String::from("? Hz");
    }
    let (scale, prefix) = [(1e9, "G"), (1e6, "M"), (1e3, "k")]
        .into_iter()
        .find(|(scale, _)| hz.abs() >= *scale)
        .unwrap_or((1.0, ""));
    let fixed = format!("{:.9}", hz / scale);
    let trimmed = if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.')
    } else {
        fixed.as_str()
    };
    format!("{trimmed} {prefix}Hz")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkDraft {
    pub label: String,
    pub mode: Option<String>,
}

#[must_use]
pub fn bookmark_draft(hz: f64, plan: Option<&BandPlan>) -> BookmarkDraft {
    let found = plan.map(|plan| identify(plan, hz)).unwrap_or_default();
    BookmarkDraft {
        label: found
            .first()
            .map_or_else(|| format_mhz(hz), |entry| entry.allocation.name.clone()),
        mode: suggested_at(&found).map(|params| params.type_id().to_owned()),
    }
}

#[must_use]
pub fn channel_type_at(
    suggested: Option<&ChannelParams>,
    listening: Option<&ChannelInfo>,
) -> String {
    suggested
        .map(ChannelParams::type_id)
        .or_else(|| listening.map(|channel| channel.settings.params.type_id()))
        .unwrap_or("nfm")
        .to_owned()
}

thread_local! {
    static AWAITING_CREATION: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
}

pub fn tune_on_create(node: &str, frequency_hz: f64) {
    AWAITING_CREATION.with(|awaiting| {
        awaiting.borrow_mut().insert(node.to_owned(), frequency_hz);
    });
}

#[must_use]
pub fn take_creation_tune(node: &str) -> Option<f64> {
    AWAITING_CREATION.with(|awaiting| awaiting.borrow_mut().remove(node))
}

#[cfg(test)]
mod tests {
    use super::{super::view::FULL_VIEW, *};

    fn channel(id: u32, stream: u32, type_id: &str) -> ChannelInfo {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "stream": stream,
            "settings": {
                "frequency_hz": 100e6,
                "params": { "type": type_id, "settings": {} }
            },
            "out_of_band": false
        }))
        .expect("a channel")
    }

    fn ids(listed: &[ChannelInfo]) -> Vec<u32> {
        listed.iter().map(|channel| channel.id).collect()
    }

    #[test]
    fn only_the_decoders_on_the_scope_stream_are_kept() {
        let listed = [
            channel(1, 0, "nfm"),
            channel(2, 1, "nfm"),
            channel(3, 0, "nfm"),
        ];
        assert_eq!(ids(&stream_channels(&listed, 0)), [1, 3]);
        assert_eq!(ids(&stream_channels(&listed, 1)), [2]);
    }

    #[test]
    fn a_drag_moves_the_centre_by_the_visible_span() {
        assert_eq!(drag_tune_hz(100e6, 2e6, FULL_VIEW, 250.0, 1000.0), 100.5e6);
        assert_eq!(drag_tune_hz(100e6, 2e6, FULL_VIEW, -250.0, 1000.0), 99.5e6);
        let half = SpectrumView {
            start: 0.25,
            end: 0.75,
        };
        assert_eq!(drag_tune_hz(100e6, 2e6, half, 500.0, 1000.0), 100.5e6);
        assert_eq!(drag_tune_hz(100e6, 0.0, FULL_VIEW, 250.0, 1000.0), 100e6);
        assert_eq!(drag_tune_hz(100e6, 2e6, FULL_VIEW, 250.0, 0.0), 100e6);
    }

    #[test]
    fn a_pick_reads_the_centre_edges_and_zoomed_window() {
        let pick = |view, at| pick_at(100e6, 2e6, view, at);
        assert_eq!(
            pick(FULL_VIEW, 0.5),
            ScopePick {
                hz: 100e6,
                offset_hz: 0.0
            }
        );
        assert_eq!(
            pick(FULL_VIEW, 0.0),
            ScopePick {
                hz: 99e6,
                offset_hz: -1e6
            }
        );
        assert_eq!(
            pick(FULL_VIEW, 1.0),
            ScopePick {
                hz: 101e6,
                offset_hz: 1e6
            }
        );
        let window = SpectrumView {
            start: 0.5,
            end: 0.75,
        };
        assert_eq!(
            pick(window, 0.5),
            ScopePick {
                hz: 100.25e6,
                offset_hz: 250_000.0
            }
        );
    }

    #[test]
    fn pick_text_carries_the_unit_and_an_ascii_sign() {
        let text = |hz, offset_hz| pick_text(ScopePick { hz, offset_hz });
        assert_eq!(
            text(156.8e6, 12_500.0),
            (String::from("156800000 Hz"), String::from("+12500 Hz"))
        );
        assert_eq!(text(99.488e6, -512_000.0).1, "-512000 Hz");
        assert_eq!(
            text(100_000_000.4, -0.4),
            (String::from("100000000 Hz"), String::from("+0 Hz"))
        );
    }

    #[test]
    fn a_bookmark_draft_names_the_allocation_or_the_frequency() {
        let plan = super::super::bands::tests::plan();
        let draft = |hz, plan| bookmark_draft(hz, plan);
        assert_eq!(
            draft(144.8e6, Some(&plan)),
            BookmarkDraft {
                label: String::from("2 m amateur"),
                mode: Some(String::from("aprs"))
            }
        );
        assert_eq!(
            draft(140e6, Some(&plan)),
            BookmarkDraft {
                label: String::from("140.0000 MHz"),
                mode: None
            }
        );
        assert_eq!(draft(156.8e6, None).label, "156.8000 MHz");
    }

    #[test]
    fn a_new_channel_prefers_the_band_then_the_current_mode_then_nfm() {
        let am: ChannelParams =
            serde_json::from_value(serde_json::json!({ "type": "am", "settings": {} }))
                .expect("am");
        let ssb = channel(1, 0, "ssb");
        assert_eq!(channel_type_at(Some(&am), Some(&ssb)), "am");
        assert_eq!(channel_type_at(None, Some(&ssb)), "ssb");
        assert_eq!(channel_type_at(None, None), "nfm");
    }

    #[test]
    fn a_frequency_reads_in_its_own_prefix_without_trailing_zeros() {
        assert_eq!(format_hz(156_800_000.0), "156.8 MHz");
        assert_eq!(format_hz(12_500.0), "12.5 kHz");
        assert_eq!(format_hz(2.4e9), "2.4 GHz");
        assert_eq!(format_hz(100.0), "100 Hz");
        assert_eq!(format_hz(f64::NAN), "? Hz");
    }

    #[test]
    fn a_creation_tune_is_handed_back_once() {
        tune_on_create("channel:abc", 12_500.0);
        assert_eq!(take_creation_tune("channel:abc"), Some(12_500.0));
        assert_eq!(take_creation_tune("channel:abc"), None);
        assert_eq!(take_creation_tune("channel:never"), None);
    }
}
