use sdrmm_wire::frame::{FusionGridFrame, SpatialSpectrumFrame, VisibilityFrame};
use sdrmm_wire::{
    AudioFrame, IqFrame, RangeDopplerFrame, SpectrumFrame, SymbolFrame, SymbolPlane, VideoData,
    VideoFrame,
};

pub(crate) fn frames() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "spectrum",
            SpectrumFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                center_hz: 100.0,
                span_hz: 48_000.0,
                db_min: -120.0,
                db_max: 0.0,
                bins: &[0, 127, 255],
            }
            .encode(),
        ),
        (
            "audio",
            AudioFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                ch_layout: 2,
                opus: &[1, 2, 3],
            }
            .encode(),
        ),
        (
            "iq",
            IqFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                center_hz: 100.0,
                sample_rate: 48_000.0,
                samples: &[0.25, -0.5],
            }
            .encode(),
        ),
        (
            "symbols",
            SymbolFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                plane: SymbolPlane::Complex,
                symbol_rate: 4800.0,
                evm: 0.25,
                mer_db: 12.0,
                margin: 4.0,
                freq_error_hz: -1.0,
                reference: &[-1.0, 1.0],
                symbols: &[0.5, -0.5],
            }
            .encode(),
        ),
        (
            "surface",
            RangeDopplerFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                ranges: 2,
                dopplers: 1,
                range_first_m: 0.0,
                range_step_m: 1_124.0,
                doppler_first_hz: -2.0,
                doppler_step_hz: 2.0,
                carrier_hz: 98_500_000.0,
                db_min: -3.0,
                db_max: 30.0,
                cells: &[1, 2],
            }
            .encode(),
        ),
        (
            "gray",
            VideoFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                width: 2,
                height: 1,
                data: VideoData::Gray(&[1, 2]),
            }
            .encode(),
        ),
        (
            "rgb",
            VideoFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                width: 2,
                height: 1,
                data: VideoData::Rgb(&[1, 2, 3, 4, 5, 6]),
            }
            .encode(),
        ),
        (
            "spatial_spectrum",
            SpatialSpectrumFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                center_hz: 433_920_000.0,
                span_hz: 2_400_000.0,
                bearings: 2,
                bins: 2,
                db_min: 0.0,
                db_max: 30.0,
                cells: &[0, 64, 128, 255],
            }
            .encode(),
        ),
        (
            "visibility",
            VisibilityFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                center_hz: 1_420_000_000.0,
                span_hz: 2_000_000.0,
                baselines: 1,
                bins: 2,
                db_min: -40.0,
                db_max: 0.0,
                amplitude: &[10, 20],
                phase: &[128, 255],
            }
            .encode(),
        ),
        (
            "fusion_grid",
            FusionGridFrame {
                stream_id: 1,
                seq: 2,
                timestamp: 3,
                south: 52.4,
                west: 13.3,
                north: 52.6,
                east: 13.5,
                cols: 2,
                rows: 1,
                cells: &[0, 255],
            }
            .encode(),
        ),
    ]
}
