use super::*;
use crate::{
    datv::dvbs2::{
        frame::{ModCod, Modulation},
        ldpc::Rate,
        receiver::{Dvbs2Decoder, Dvbs2Encoder, Dvbs2Output},
    },
    testutil::realtime_budget,
};

#[test]
fn superframe_headers_survive_phase_frequency_and_noise() {
    let mut state = 17931u32;
    for code in 0..16 {
        let mut symbols = wrap(&[Complex::new(0.0, 0.0)], code, false, 1);
        for (i, symbol) in symbols[..HEADER].iter_mut().enumerate() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *symbol *= Complex::from_polar(1.0, 0.7 + i as f32 * 0.002);
            *symbol += Complex::new((state as i32 as f32) / i32::MAX as f32 * 0.02, 0.0);
        }
        let known = sequence();
        let (phase, frequency) = fit(&symbols[..SOSF], &known[..SOSF]).expect("SOSF fit");
        assert!((phase - 0.7).abs() < 0.02);
        assert!((frequency - 0.002).abs() < 0.0002);
        assert_eq!(
            format(&symbols[..HEADER], &known[..HEADER], phase, frequency),
            Some(code)
        );
    }
}

#[test]
fn legacy_and_extended_payloads_cross_superframe_boundaries_with_both_pilot_settings() {
    for (code, modcod) in [
        (1, ModCod::find(Modulation::Qpsk, Rate::R1_2).unwrap()),
        (0, ModCod::from_index(214).unwrap()),
    ] {
        for pilots in [false, true] {
            let mut encoder = Dvbs2Encoder::new(modcod, false, false).unwrap();
            let packets = vec![crate::datv::ts::null_packet(); encoder.capacity()];
            let mut payload = Vec::new();
            assert!(encoder.frame(&packets, &mut payload));
            payload.clear();
            assert!(encoder.frame(&packets, &mut payload));
            let mut header = Vec::new();
            pl::header(
                pl::Signalling {
                    modcod: modcod.index,
                    short: false,
                    pilots,
                },
                &mut header,
            );
            payload[..pl::HEADER].copy_from_slice(&header);
            let mut signal = wrap(&payload, code, pilots, 2);
            for (i, sample) in signal.iter_mut().enumerate() {
                *sample *= Complex::from_polar(1.0, 0.43 + 0.0001 * i as f32);
            }
            let mut decoder = Dvbs2Decoder::new();
            decoder.superframes(true);
            let mut decoded = Dvbs2Output::default();
            for chunk in signal.chunks(8191) {
                decoder.push(chunk, &mut decoded);
            }
            assert!(
                decoder.metrics.frames_ok > 20,
                "format {code}, pilots {pilots}: {:?}",
                decoder.metrics
            );
            assert_eq!(
                decoder.metrics.frames_bad, 0,
                "format {code}, pilots {pilots}"
            );
            assert!(
                decoded
                    .packets
                    .iter()
                    .all(|packet| packet == &crate::datv::ts::null_packet())
            );
            assert_eq!(decoder.superframe_format(), Some(code));
        }
    }
}

#[test]
fn unsupported_formats_are_reported_and_do_not_feed_payload_to_the_decoder() {
    let symbols = wrap(&[Complex::new(1.0, 0.0)], 2, true, 1);
    let mut decoder = Dvbs2Decoder::new();
    decoder.superframes(true);
    let mut out = Dvbs2Output::default();
    for chunk in symbols.chunks(4001) {
        decoder.push(chunk, &mut out);
    }
    assert_eq!(decoder.superframe_format(), Some(2));
    assert_eq!(decoder.metrics.frames_skipped, 1);
    assert_eq!(decoder.metrics.frames_ok, 0);
    assert!(out.packets.is_empty());
}

#[test]
fn very_low_snr_superframes_use_ninety_symbol_pilots_and_short_frame_padding() {
    use crate::datv::dvbs2::vlsnr::{VlMode, VlSet, VlSnrEncoder};
    assert_eq!(
        crate::datv::dvbs2::vlsnr::superframe_symbols(VlSet::One),
        33_660
    );
    assert_eq!(
        crate::datv::dvbs2::vlsnr::superframe_symbols(VlSet::Two),
        16_920
    );
    for index in [0, 9] {
        let mode = VlMode::from_header(index).unwrap();
        let mut encoder = VlSnrEncoder::new(mode).unwrap();
        encoder.superframes(true);
        let mut payload = Vec::new();
        let packets = vec![crate::datv::ts::null_packet(); encoder.capacity()];
        assert!(encoder.frame(&packets, &mut payload));
        payload.clear();
        assert!(encoder.frame(&packets, &mut payload));
        let signal = wrap(&payload, 0, true, 1);
        let mut decoder = Dvbs2Decoder::new();
        decoder.superframes(true);
        let mut output = Dvbs2Output::default();
        for chunk in signal.chunks(7129) {
            decoder.push(chunk, &mut output);
        }
        assert!(decoder.metrics.frames_ok > 15, "{:?}", decoder.metrics);
        assert_eq!(decoder.metrics.frames_bad, 0);
        assert!(
            output
                .packets
                .iter()
                .all(|packet| packet == &crate::datv::ts::null_packet())
        );
    }
}

#[test]
fn independent_superframe_recording_delivers_expected_packets_in_real_time() {
    let data = include_bytes!("../../../../../../fixtures/dvbs2x/superframe0.sigmf-data");
    let symbols: Vec<_> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|sample| {
            Complex::new(
                f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 16000.0,
                f32::from(i16::from_le_bytes([sample[2], sample[3]])) / 16000.0,
            )
        })
        .collect();
    let mut receiver = Dvbs2Decoder::new();
    receiver.superframes(true);
    let mut output = Dvbs2Output::default();
    let start = std::time::Instant::now();
    for chunk in symbols.chunks(4037) {
        receiver.push(chunk, &mut output);
    }
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(output.packets.len(), 72 * 32 - 1);
    assert_eq!(receiver.metrics.frames_ok, 72, "{:?}", receiver.metrics);
    assert_eq!(receiver.metrics.frames_bad, 0);
    assert_eq!(receiver.metrics.transport_errors, 0);
    for (index, packet) in output.packets.iter().enumerate() {
        let index = index % 32;
        assert_eq!(&packet[..4], &[0x47, 1, 0x23, 0x10 | (index & 15) as u8]);
        for (byte, &value) in packet[4..].iter().enumerate() {
            assert_eq!(value, (214 + index + byte) as u8);
        }
    }
    let duration = symbols.len() as f64 / crate::testgen::datv::SYMBOL_RATE;
    assert!(
        elapsed < realtime_budget(duration),
        "{duration:.3}s of IQ took {elapsed:.3}s to decode"
    );
}
