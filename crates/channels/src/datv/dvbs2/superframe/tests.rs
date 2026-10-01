use super::{
    synth::{Codes, Dwell, Options, Plframe, Transmitter},
    *,
};
use crate::{
    datv::{
        dvbs::PACKET,
        dvbs2::{
            frame::{ModCod, Modulation},
            ldpc::{Frame, Rate},
            receiver::{Dvbs2Decoder, Dvbs2Encoder, Dvbs2Output},
            vlsnr::Carrier,
            xfec::Xfec,
        },
    },
    testutil::realtime_budget,
};
use coding::Coding;

mod formats;

fn transport(count: usize, seed: u32) -> Vec<[u8; PACKET]> {
    let mut state = seed | 1;
    (0..count)
        .map(|index| {
            let mut packet = [0u8; PACKET];
            packet[0] = 0x47;
            packet[1] = 0x01;
            packet[2] = index as u8;
            for byte in &mut packet[3..] {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            packet
        })
        .collect()
}

struct Source {
    carry: u8,
    seed: u32,
    sent: Vec<[u8; PACKET]>,
}

impl Source {
    fn new() -> Self {
        Self {
            carry: 0x47,
            seed: 91,
            sent: Vec::new(),
        }
    }

    fn frame(&mut self, coding: Coding, compact: bool) -> Vec<Complex<f32>> {
        let codec = Xfec::new(coding).unwrap_or_else(|| panic!("{coding:?}"));
        let packets = transport(codec.baseband.capacity(), self.seed);
        self.seed += 2;
        let baseband = codec
            .baseband
            .build(&packets, &mut self.carry)
            .expect("a base band frame");
        self.sent.extend(packets);
        let mut out = Vec::new();
        codec.encode(&baseband, compact, &mut out);
        out
    }

    fn bundle(&mut self, format: u8, code: u8) -> (u8, Vec<Complex<f32>>) {
        let Some(coding::Signal::Data(coding)) = coding::bundle(format, code) else {
            return (code, Vec::new());
        };
        let size = layout::bundles(format).expect("a bundled format").payload;
        let frames = coding.bundled(size).expect("whole frames");
        let codec = Xfec::new(coding).unwrap_or_else(|| panic!("{coding:?}"));
        let basebands: Vec<Vec<bool>> = (0..frames)
            .map(|_| {
                let packets = transport(codec.baseband.capacity(), self.seed);
                self.seed += 2;
                let baseband = codec
                    .baseband
                    .build(&packets, &mut self.carry)
                    .expect("a base band frame");
                self.sent.extend(packets);
                baseband
            })
            .collect();
        let mut payload = Vec::with_capacity(size);
        codec.encode_bundle(&basebands, &mut payload);
        (code, payload)
    }

    fn plframe(&mut self, coding: Coding, spread: usize) -> Plframe {
        Plframe::data(coding, spread, self.frame(coding, false))
    }
}

fn impair(signal: &mut [Complex<f32>], frequency: f32, noise: f32, seed: u32) {
    let mut state = seed | 1;
    let mut uniform = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    };
    for (index, symbol) in signal.iter_mut().enumerate() {
        let gauss = |uniform: &mut dyn FnMut() -> f32| {
            (uniform() + uniform() + uniform() + uniform()) * std::f32::consts::SQRT_2
        };
        let re = gauss(&mut uniform);
        let im = gauss(&mut uniform);
        *symbol = *symbol * Complex::from_polar(1.0, 0.6 + frequency * index as f32)
            + Complex::new(re, im) * noise;
    }
}

fn receive(signal: &[Complex<f32>], settings: Settings) -> (Dvbs2Decoder, Dvbs2Output) {
    let mut decoder = Dvbs2Decoder::new();
    decoder.superframes(true);
    decoder.configure_superframes(settings);
    let mut output = Dvbs2Output::default();
    for chunk in signal.chunks(8191) {
        decoder.push(chunk, &mut output);
    }
    (decoder, output)
}

fn receive_flushed(signal: &[Complex<f32>], settings: Settings) -> (Dvbs2Decoder, Dvbs2Output) {
    let mut padded = signal.to_vec();
    padded.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), 4096));
    receive(&padded, settings)
}

fn assert_delivered(source: &Source, decoder: &Dvbs2Decoder, output: &Dvbs2Output, what: &str) {
    let sent = &source.sent;
    assert_eq!(
        decoder.metrics.frames_bad, 0,
        "{what}: {:?}",
        decoder.metrics
    );
    assert_eq!(
        output.packets.len(),
        sent.len() - 1,
        "{what}: {:?}",
        decoder.metrics
    );
    assert_eq!(output.packets, sent[..sent.len() - 1], "{what}");
}

#[test]
fn superframe_headers_survive_phase_frequency_and_noise() {
    let gold = Gold::new();
    let mut state = 17931u32;
    for (format, sosf) in [(0u8, 0u8), (1, 5), (5, 255), (7, 128), (15, 77)] {
        for (reference, payload) in [(0u32, 0u32), (99, 1_000_001)] {
            let codes = Codes {
                reference,
                payload,
                sosf,
                ..Codes::default()
            };
            let mut symbols =
                Transmitter::new(codes).legacy(&[Complex::new(0.0, 0.0)], format, false, 1);
            for (i, symbol) in symbols[..HEADER].iter_mut().enumerate() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *symbol *= Complex::from_polar(1.0, 0.7 + i as f32 * 0.002);
                *symbol += Complex::new((state as i32 as f32) / i32::MAX as f32 * 0.02, 0.0);
            }
            let found = detect::detect(
                &symbols[..HEADER],
                &Sequence::new(&gold, reference),
                &Sequence::new(&gold, payload),
            )
            .expect("a superframe header");
            assert!((found.phase - 0.7).abs() < 0.02);
            assert!((found.frequency - 0.002).abs() < 0.0002);
            assert_eq!(found.format, format);
            assert_eq!(found.sosf, sosf);
            if reference != 0 {
                assert!(
                    detect::detect(
                        &symbols[..HEADER],
                        &Sequence::new(&gold, 0),
                        &Sequence::new(&gold, payload),
                    )
                    .is_none()
                );
            }
        }
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
            let (decoder, decoded) = receive(&signal, Settings::default());
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
fn reserved_formats_are_reported_and_do_not_feed_payload_to_the_decoder() {
    let symbols = wrap(&[Complex::new(1.0, 0.0)], 9, true, 1);
    let (decoder, out) = receive(&symbols, Settings::default());
    assert_eq!(decoder.superframe_format(), Some(9));
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
fn independent_superframe_recording_keeps_ahead_of_realtime() {
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
    let duration = symbols.len() as f64 / crate::synth::datv::SYMBOL_RATE;
    assert!(
        elapsed < realtime_budget(duration),
        "{duration:.3}s of IQ took {elapsed:.3}s to decode"
    );
}
