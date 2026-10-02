use super::{encode::drm_frame, *};

pub(crate) fn access_units(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    while bytes.len() >= 2 {
        let size = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
        units.push(&bytes[2..2 + size]);
        bytes = &bytes[2 + size..];
    }
    units
}

pub(crate) fn config(sbr: bool, mode: AudioMode, rate_hz: u32) -> AudioConfig {
    AudioConfig {
        coding: Coding::Aac,
        sbr,
        mode,
        rate_hz,
        rate_code: 0,
        text: false,
        surround: 0,
        config: [0; MAX_CONFIG],
        config_length: 0,
    }
}

fn fixtures() -> [(&'static [u8], AudioConfig); 4] {
    [
        (
            include_bytes!("../../../../../fixtures/drm/drm_he_mono_24k.aus"),
            config(true, AudioMode::Mono, 12_000),
        ),
        (
            include_bytes!("../../../../../fixtures/drm/drm_he_ps_24k.aus"),
            config(true, AudioMode::ParametricStereo, 12_000),
        ),
        (
            include_bytes!("../../../../../fixtures/drm/drm_lc_stereo_24k.aus"),
            config(false, AudioMode::Stereo, 24_000),
        ),
        (
            include_bytes!("../../../../../fixtures/drm/drm_lc_mono_48k.aus"),
            config(false, AudioMode::Mono, 48_000),
        ),
    ]
}

fn spectrum(element: &encode::Element) -> Vec<(u8, u16, u64, u8)> {
    element
        .channels
        .iter()
        .flat_map(|channel| {
            let mut words: Vec<_> = channel
                .words
                .iter()
                .map(|word| (word.window, word.line, word.bits, word.length))
                .collect();
            words.sort_unstable();
            words
        })
        .collect()
}

#[test]
fn drm_frames_convert_back_to_the_same_spectrum() {
    for (source, config) in fixtures() {
        for unit in access_units(source) {
            let frame = drm_frame(unit, &config).expect("a DRM frame");
            let block = standard(&frame, &config).expect("a standard block");
            let original = encode::read_element(unit, &config).expect("source parses");
            let converted = encode::read_element(&block, &config).expect("result parses");
            assert_eq!(spectrum(&original), spectrum(&converted));
            assert_eq!(original.info, converted.info);
            assert_eq!(original.ms, converted.ms);
            assert!(converted.sbr.starts_with(&original.sbr));
            assert_eq!(config.sbr, !original.sbr.is_empty());
        }
    }
}

#[test]
fn a_flipped_side_information_bit_fails_the_frame_crc() {
    let (source, config) = fixtures()[0];
    let unit = access_units(source)[3];
    let mut frame = drm_frame(unit, &config).expect("a DRM frame");
    frame[2] ^= 0x01;
    assert_eq!(standard(&frame, &config), Err("DRM AAC frame CRC failure"));
}

#[test]
fn damaged_spectral_data_is_reported() {
    let (source, config) = fixtures()[2];
    let mut damaged = 0;
    for unit in access_units(source) {
        let mut frame = drm_frame(unit, &config).expect("a DRM frame");
        let end = frame.len() - 1;
        for byte in &mut frame[end - 8..end] {
            *byte ^= 0xA5;
        }
        if standard(&frame, &config).is_err() {
            damaged += 1;
        }
    }
    assert!(damaged > 0);
}

#[test]
#[ignore = "rewrites fixtures/drm/*.drm"]
fn regenerate_drm_frame_fixtures() {
    let names = [
        "drm_he_mono_24k",
        "drm_he_ps_24k",
        "drm_lc_stereo_24k",
        "drm_lc_mono_48k",
    ];
    for ((source, config), name) in fixtures().into_iter().zip(names) {
        let mut bytes = Vec::new();
        for unit in access_units(source) {
            let frame = drm_frame(unit, &config).expect("a DRM frame");
            bytes.extend_from_slice(&(frame.len() as u16).to_be_bytes());
            bytes.extend_from_slice(&frame);
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/drm")
            .join(format!("{name}.drm"));
        std::fs::write(path, bytes).expect("fixture written");
    }
}
