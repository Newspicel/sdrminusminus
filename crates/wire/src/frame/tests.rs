use super::*;
use crate::radar::{SURFACE_DB_MAX, SURFACE_DB_MIN};

fn spectrum(bins: &[u8]) -> SpectrumFrame<'_> {
    SpectrumFrame {
        stream_id: 7,
        seq: 42,
        timestamp: 1_000_000,
        center_hz: 100_300_000.0,
        span_hz: 2_400_000.0,
        db_min: -120.0,
        db_max: -20.0,
        bins,
    }
}

fn range_doppler() -> RangeDopplerOwned {
    RangeDopplerOwned {
        stream_id: 9,
        seq: 3,
        timestamp: 4_096,
        ranges: 4,
        dopplers: 3,
        range_first_m: 0.0,
        range_step_m: 1_124.0,
        doppler_first_hz: -2.0,
        doppler_step_hz: 2.0,
        carrier_hz: 98.5e6,
        db_min: SURFACE_DB_MIN,
        db_max: SURFACE_DB_MAX,
        cells: (0..12).collect(),
    }
}

fn spatial() -> SpatialSpectrumOwned {
    SpatialSpectrumOwned {
        stream_id: 2,
        seq: 5,
        timestamp: 77,
        center_hz: 433.92e6,
        span_hz: 2.4e6,
        bearings: 3,
        bins: 4,
        db_min: 0.0,
        db_max: 30.0,
        cells: (0..12).map(|i| i * 20).collect(),
    }
}

fn visibility() -> VisibilityOwned {
    VisibilityOwned {
        stream_id: 3,
        seq: 6,
        timestamp: 88,
        center_hz: 1.42e9,
        span_hz: 2.0e6,
        baselines: 2,
        bins: 3,
        db_min: -40.0,
        db_max: 0.0,
        amplitude: vec![1, 2, 3, 4, 5, 6],
        phase: vec![128, 0, 255, 64, 32, 16],
    }
}

fn fusion_grid() -> FusionGridOwned {
    FusionGridOwned {
        stream_id: 4,
        seq: 7,
        timestamp: 99,
        south: 52.4,
        west: 13.3,
        north: 52.6,
        east: 13.5,
        cols: 2,
        rows: 2,
        cells: vec![0, 64, 128, 255],
    }
}

fn surfaces() -> [SurfaceFrame; 4] {
    [
        SurfaceFrame::RangeDoppler(range_doppler()),
        SurfaceFrame::SpatialSpectrum(spatial()),
        SurfaceFrame::Visibility(visibility()),
        SurfaceFrame::FusionGrid(fusion_grid()),
    ]
}

#[test]
fn spectrum_roundtrip() {
    let bins: Vec<u8> = (0..64u16).map(|i| (i * 4) as u8).collect();
    let frame = spectrum(&bins);
    let buf = frame.encode();
    assert_eq!(buf.len(), frame.encoded_len());
    assert_eq!(SpectrumFrame::decode(&buf), Ok(frame));
}

#[test]
fn audio_roundtrip_in_both_layouts() {
    let opus: Vec<u8> = (0..96u8).map(|i| i.wrapping_mul(3)).collect();
    for ch_layout in [1u8, 2] {
        let frame = AudioFrame {
            stream_id: 3,
            seq: 512,
            timestamp: 96_000,
            ch_layout,
            opus: &opus,
        };
        let buf = frame.encode();
        assert_eq!(buf.len(), frame.encoded_len());
        assert_eq!(AudioFrame::decode(&buf), Ok(frame));
    }
}

#[test]
fn symbols_roundtrip_carrying_both_the_cloud_and_its_reference() {
    let reference: Vec<f32> = vec![0.707, 0.707, -0.707, 0.707, -0.707, -0.707, 0.707, -0.707];
    let symbols: Vec<f32> = (0..128).map(|i| (i as f32) * 0.01 - 0.64).collect();
    let frame = SymbolFrame {
        stream_id: 11,
        seq: 9,
        timestamp: 4096,
        plane: SymbolPlane::Complex,
        symbol_rate: 4800.0,
        evm: 0.083,
        mer_db: 21.6,
        margin: 3.2,
        freq_error_hz: -12.5,
        reference: &reference,
        symbols: &symbols,
    };
    let buf = frame.encode();
    assert_eq!(buf.len(), frame.encoded_len());
    let header = peek_header(&buf).expect("header");
    assert_eq!(header.kind, FrameKind::Symbols);
    assert_eq!(
        (header.stream_id, header.seq, header.timestamp),
        (11, 9, 4096)
    );
    assert_eq!(SymbolPlane::from_u8(buf[16]), Some(SymbolPlane::Complex));
    assert_eq!(f32::from_le_bytes(buf[17..21].try_into().unwrap()), 4800.0);
    assert_eq!(f32::from_le_bytes(buf[33..37].try_into().unwrap()), -12.5);
    let count = u16::from_le_bytes([buf[37], buf[38]]) as usize;
    assert_eq!(count, reference.len());
    let floats: Vec<f32> = buf[39..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&c| f32::from_le_bytes(c))
        .collect();
    assert_eq!(&floats[..count], reference.as_slice());
    assert_eq!(&floats[count..], symbols.as_slice());
}

#[test]
fn a_level_plane_frame_needs_no_reference_pairs() {
    let symbols: Vec<f32> = vec![1.0, 0.33, -0.33, -1.0];
    let frame = SymbolFrame {
        stream_id: 1,
        seq: 0,
        timestamp: 0,
        plane: SymbolPlane::Level,
        symbol_rate: 4800.0,
        evm: 0.0,
        mer_db: 0.0,
        margin: 0.0,
        freq_error_hz: 0.0,
        reference: &symbols,
        symbols: &symbols,
    };
    let buf = frame.encode();
    assert_eq!(buf.len(), frame.encoded_len());
    assert_eq!(SymbolPlane::from_u8(buf[16]), Some(SymbolPlane::Level));
}

#[test]
fn iq_roundtrip() {
    let samples: Vec<f32> = (0..32).map(|i| i as f32 * 0.03125 - 0.5).collect();
    let frame = IqFrame {
        stream_id: 0x8100,
        seq: 3,
        timestamp: 48_000,
        sample_rate: 24_000.0,
        center_hz: 145_800_000.0,
        samples: &samples,
    };
    let buf = frame.encode();
    assert_eq!(buf.len(), frame.encoded_len());
    let header = peek_header(&buf).expect("header");
    assert_eq!(header.kind, FrameKind::IqF32);
    assert_eq!(
        f64::from_le_bytes(buf[16..24].try_into().unwrap()),
        145_800_000.0
    );
    assert_eq!(
        f32::from_le_bytes(buf[24..28].try_into().unwrap()),
        24_000.0
    );
    let (chunks, _) = buf[28..].as_chunks::<4>();
    let out: Vec<f32> = chunks.iter().copied().map(f32::from_le_bytes).collect();
    assert_eq!(out, samples);
}

#[test]
fn video_roundtrip() {
    let luma: Vec<u8> = (0..(8u32 * 4)).map(|i| (i * 7) as u8).collect();
    let frame = VideoFrame {
        stream_id: 0x8001,
        seq: 9,
        timestamp: 2_000_000,
        width: 8,
        height: 4,
        data: VideoData::Gray(&luma),
    };
    let buf = frame.encode();
    assert_eq!(buf.len(), frame.encoded_len());
    assert_eq!(VideoFrame::decode(&buf), Ok(frame));
}

#[test]
fn rgb_video_roundtrip() {
    let rgb: Vec<u8> = (0..(3 * 3 * 2)).map(|i| (i * 11) as u8).collect();
    let frame = VideoFrame {
        stream_id: 12,
        seq: 4,
        timestamp: 99,
        width: 3,
        height: 2,
        data: VideoData::Rgb(&rgb),
    };
    let buf = frame.encode();
    let decoded = VideoFrame::decode(&buf).expect("decode");
    assert_eq!(decoded.data, VideoData::Rgb(&rgb));
    let mut short = buf.clone();
    short.pop();
    assert_eq!(VideoFrame::decode(&short), Err(FrameError::Shape));
}

#[test]
fn a_range_doppler_surface_encodes_its_shape_ahead_of_its_cells() {
    let owned = range_doppler();
    let buf = owned.frame().encode();
    assert_eq!(buf.len(), owned.frame().encoded_len());
    assert_eq!(FrameKind::from_u8(buf[1]), Some(FrameKind::RangeDoppler));
    assert_eq!(u16::from_le_bytes([buf[16], buf[17]]), 4);
    assert_eq!(u16::from_le_bytes([buf[18], buf[19]]), 3);
    assert_eq!(f32::from_le_bytes(buf[24..28].try_into().unwrap()), 1_124.0);
    assert_eq!(f64::from_le_bytes(buf[36..44].try_into().unwrap()), 98.5e6);
    assert_eq!(&buf[52..], owned.cells.as_slice());
}

#[test]
fn range_doppler_frame_round_trips() {
    let owned = range_doppler();
    let frame = owned.frame();
    assert_eq!(RangeDopplerFrame::decode(&frame.encode()), Ok(frame));
    let surface = SurfaceFrame::RangeDoppler(owned.clone());
    assert_eq!(surface.encode(9), owned.frame().encode());
}

#[test]
fn range_doppler_frame_decode_checks_shape() {
    let owned = range_doppler();
    let buf = owned.frame().encode();
    assert_eq!(RangeDopplerFrame::decode(&buf), Ok(owned.frame()));
    let mut short = buf.clone();
    short.pop();
    assert_eq!(RangeDopplerFrame::decode(&short), Err(FrameError::Shape));
    let empty = RangeDopplerOwned {
        ranges: 0,
        dopplers: 0,
        cells: Vec::new(),
        ..range_doppler()
    };
    assert_eq!(
        RangeDopplerFrame::decode(&empty.frame().encode()),
        Err(FrameError::Shape)
    );
}

#[test]
fn spatial_visibility_and_fusion_frames_round_trip() {
    let spatial = spatial();
    let buf = spatial.frame().encode();
    assert_eq!(SpatialSpectrumFrame::decode(&buf), Ok(spatial.frame()));
    let visibility = visibility();
    let buf = visibility.frame().encode();
    assert_eq!(VisibilityFrame::decode(&buf), Ok(visibility.frame()));
    let grid = fusion_grid();
    let buf = grid.frame().encode();
    assert_eq!(FusionGridFrame::decode(&buf), Ok(grid.frame()));
}

#[test]
fn a_frame_with_short_cells_is_refused() {
    let mut spatial = spatial();
    spatial.cells.pop();
    assert_eq!(
        SpatialSpectrumFrame::decode(&spatial.frame().encode()),
        Err(FrameError::Shape)
    );
    let mut visibility = visibility();
    visibility.phase.pop();
    assert_eq!(
        VisibilityFrame::decode(&visibility.frame().encode()),
        Err(FrameError::Shape)
    );
    let big = FusionGridOwned {
        cols: FUSION_FRAME_CELLS + 1,
        rows: 1,
        cells: vec![0; usize::from(FUSION_FRAME_CELLS) + 1],
        ..fusion_grid()
    };
    assert_eq!(
        FusionGridFrame::decode(&big.frame().encode()),
        Err(FrameError::Shape)
    );
}

#[test]
fn every_decodable_frame_round_trips() {
    let bins = [0u8, 127, 255];
    let spectrum = spectrum(&bins);
    assert_eq!(SpectrumFrame::decode(&spectrum.encode()), Ok(spectrum));
    let opus = [1u8, 2, 3];
    let audio = AudioFrame {
        stream_id: 1,
        seq: 2,
        timestamp: 3,
        ch_layout: 2,
        opus: &opus,
    };
    assert_eq!(AudioFrame::decode(&audio.encode()), Ok(audio));
    let pixels = [1u8, 2];
    let video = VideoFrame {
        stream_id: 1,
        seq: 2,
        timestamp: 3,
        width: 2,
        height: 1,
        data: VideoData::Gray(&pixels),
    };
    assert_eq!(VideoFrame::decode(&video.encode()), Ok(video));
    for surface in surfaces() {
        let buf = surface.encode(31);
        let header = peek_header(&buf).expect("header");
        assert_eq!(header.stream_id, 31);
        let decoded = match &surface {
            SurfaceFrame::RangeDoppler(owned) => RangeDopplerFrame::decode(&buf).map(|frame| {
                frame
                    == RangeDopplerFrame {
                        stream_id: 31,
                        ..owned.frame()
                    }
            }),
            SurfaceFrame::SpatialSpectrum(owned) => {
                SpatialSpectrumFrame::decode(&buf).map(|frame| {
                    frame
                        == SpatialSpectrumFrame {
                            stream_id: 31,
                            ..owned.frame()
                        }
                })
            }
            SurfaceFrame::Visibility(owned) => VisibilityFrame::decode(&buf).map(|frame| {
                frame
                    == VisibilityFrame {
                        stream_id: 31,
                        ..owned.frame()
                    }
            }),
            SurfaceFrame::FusionGrid(owned) => FusionGridFrame::decode(&buf).map(|frame| {
                frame
                    == FusionGridFrame {
                        stream_id: 31,
                        ..owned.frame()
                    }
            }),
        };
        assert_eq!(decoded, Ok(true), "{:?}", surface.kind());
    }
}

#[test]
fn surfaces_name_their_stream_kind() {
    let kinds: Vec<StreamKind> = surfaces().iter().map(SurfaceFrame::kind).collect();
    assert_eq!(
        kinds,
        [
            StreamKind::RangeDoppler,
            StreamKind::SpatialSpectrum,
            StreamKind::Visibility,
            StreamKind::FusionGrid
        ]
    );
}

#[test]
fn a_short_frame_is_short() {
    let bins = [1u8, 2, 3];
    let buf = spectrum(&bins).encode();
    for len in 0..HEADER_LEN {
        assert_eq!(SpectrumFrame::decode(&buf[..len]), Err(FrameError::Short));
        assert_eq!(peek_header(&buf[..len]), Err(FrameError::Short));
    }
    assert_eq!(SpectrumFrame::decode(&buf[..20]), Err(FrameError::Short));
    let grid = fusion_grid().frame().encode();
    assert_eq!(FusionGridFrame::decode(&grid[..30]), Err(FrameError::Short));
}

#[test]
fn a_wrong_kind_is_refused() {
    let bins = [1u8, 2, 3];
    let buf = spectrum(&bins).encode();
    assert_eq!(
        RangeDopplerFrame::decode(&buf),
        Err(FrameError::Kind(FrameKind::Spectrum as u8))
    );
    let mut unknown = buf.clone();
    unknown[1] = 200;
    assert_eq!(peek_header(&unknown), Err(FrameError::Kind(200)));
    let mut version = buf;
    version[0] = 9;
    assert_eq!(SpectrumFrame::decode(&version), Err(FrameError::Version(9)));
    assert_eq!(peek_header(&version), Err(FrameError::Version(9)));
}

#[test]
fn a_bad_bytes16_length_is_length() {
    let bins = [1u8, 2, 3];
    let mut buf = spectrum(&bins).encode();
    buf[36] = 9;
    assert_eq!(SpectrumFrame::decode(&buf), Err(FrameError::Length));
    let mut trailing = spectrum(&bins).encode();
    trailing.push(0);
    assert_eq!(SpectrumFrame::decode(&trailing), Err(FrameError::Length));
    let mut visibility = visibility().frame().encode();
    let at = HEADER_LEN + 8 + 4 + 2 + 2 + 4 + 4;
    visibility[at] = 200;
    assert_eq!(
        VisibilityFrame::decode(&visibility),
        Err(FrameError::Length)
    );
}

#[test]
fn peek_header_reads_without_decoding() {
    let mut buf = range_doppler().frame().encode();
    buf.truncate(HEADER_LEN);
    let header = peek_header(&buf).expect("header");
    assert_eq!(
        header,
        FrameHeader {
            kind: FrameKind::RangeDoppler,
            stream_id: 9,
            seq: 3,
            timestamp: 4_096,
        }
    );
    assert_eq!(RangeDopplerFrame::decode(&buf), Err(FrameError::Short));
}

#[test]
fn every_frame_kind_survives_the_byte_it_is_written_as() {
    for (byte, kind) in [
        (0u8, FrameKind::Spectrum),
        (1, FrameKind::AudioOpus),
        (2, FrameKind::IqF32),
        (3, FrameKind::VideoGray),
        (4, FrameKind::VideoRgb),
        (5, FrameKind::Symbols),
        (6, FrameKind::RangeDoppler),
        (7, FrameKind::SpatialSpectrum),
        (8, FrameKind::Visibility),
        (9, FrameKind::FusionGrid),
    ] {
        assert_eq!(FrameKind::from_u8(byte), Some(kind));
        assert_eq!(kind as u8, byte);
    }
    assert_eq!(FrameKind::from_u8(10), None);
}

#[test]
fn typescript_frames_include_the_new_kinds() {
    let ts = typescript_frames();
    for needle in [
        "export const FRAME_KIND_SPATIAL_SPECTRUM = 7;",
        "export const FRAME_KIND_VISIBILITY = 8;",
        "export const FRAME_KIND_FUSION_GRID = 9;",
        "export function decodeSpatialSpectrum(",
        "export function decodeVisibility(",
        "export function decodeFusionGrid(",
        "export interface RangeDopplerFrame",
        "rangeFirstM: number;",
        "carrierHz: number;",
        "reader.bytes(bearings * bins)",
        "reader.bytes(baselines * bins)",
        "reader.bytes(cols * rows)",
        "if (cols > FUSION_FRAME_CELLS || rows > FUSION_FRAME_CELLS) return null;",
        "export const WS_SUBPROTOCOL = \"sdrmm\";",
        "export const WS_BEARER_PROTOCOL_PREFIX = \"sdrmm.bearer.\";",
    ] {
        assert!(ts.contains(needle), "missing {needle}");
    }
    assert!(!ts.contains("rangeStepUs"));
}
