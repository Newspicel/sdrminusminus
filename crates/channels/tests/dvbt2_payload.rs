use num_complex::Complex;
use sdrmm_channels::dvbt2::{Coding, Constellation, DecodeError, Frame, Rate, bicm::Decoder};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn reference(bytes: &[u8]) -> Vec<Complex<f32>> {
    let (chunks, remainder) = bytes.as_chunks::<8>();
    assert!(remainder.is_empty());
    chunks
        .iter()
        .map(|&[a, b, c, d, e, f, g, h]| {
            Complex::new(
                f32::from_le_bytes([a, b, c, d]),
                f32::from_le_bytes([e, f, g, h]),
            )
        })
        .collect()
}

#[test]
fn independent_etsi_payload_vectors_decode_without_allocations() {
    for (frame, rate, constellation, rotated, bytes) in [
        (
            Frame::Normal,
            Rate::R2_3,
            Constellation::Qam256,
            true,
            include_bytes!("../../../fixtures/dvbt2/normal_2_3_qam256.f32").as_slice(),
        ),
        (
            Frame::Normal,
            Rate::R3_5,
            Constellation::Qam64,
            false,
            include_bytes!("../../../fixtures/dvbt2/normal_3_5_qam64.f32").as_slice(),
        ),
        (
            Frame::Short,
            Rate::R3_5,
            Constellation::Qam64,
            true,
            include_bytes!("../../../fixtures/dvbt2/short_3_5_qam64.f32").as_slice(),
        ),
        (
            Frame::Short,
            Rate::R1_3,
            Constellation::Qam16,
            true,
            include_bytes!("../../../fixtures/dvbt2/lite_1_3_qam16.f32").as_slice(),
        ),
    ] {
        let coding = Coding {
            frame,
            rate,
            constellation,
            rotated,
            lite: rate == Rate::R1_3,
        };
        let mut decoder = Decoder::new(coding).unwrap();
        let mut cells = reference(bytes);
        let expected: Vec<_> = (0..coding.message())
            .map(|i| (i * 173 + i / 7) % 31 < 15)
            .collect();
        let mut output = vec![false; coding.message()];
        assert_no_alloc("DVB-T2 clean payload", || {
            let decoded = decoder.decode(&cells, 3, 0.01, &mut output).unwrap();
            assert_eq!(decoded.bits, expected.len());
        });
        assert_eq!(output, expected, "{coding:?}");
        for i in (11..cells.len()).step_by(401) {
            cells[i] = -cells[i];
        }
        assert_no_alloc("DVB-T2 damaged payload", || {
            let decoded = decoder.decode(&cells, 3, 0.01, &mut output).unwrap();
            assert!(decoded.ldpc_iterations > 0);
        });
        assert_eq!(output, expected, "{coding:?}");
        cells.fill(Complex::new(0.5, -0.5));
        assert!(matches!(
            decoder.decode(&cells, 3, 0.01, &mut output),
            Err(DecodeError::Ldpc | DecodeError::Bch)
        ));
    }
}

#[test]
fn outer_bch_correction_uses_preallocated_scratch() {
    let coding = Coding {
        frame: Frame::Short,
        rate: Rate::R3_5,
        constellation: Constellation::Qam64,
        rotated: true,
        lite: false,
    };
    let cells = reference(include_bytes!(
        "../../../fixtures/dvbt2/short_3_5_bch_errors.f32"
    ));
    let mut decoder = Decoder::new(coding).unwrap();
    let mut output = vec![false; coding.message()];
    assert_no_alloc("DVB-T2 BCH correction", || {
        let decoded = decoder.decode(&cells, 3, 0.01, &mut output).unwrap();
        assert_eq!(decoded.ldpc_iterations, 0);
        assert_eq!(decoded.corrected_bits, 2);
    });
    for (i, bit) in output.into_iter().enumerate() {
        assert_eq!(bit, (i * 173 + i / 7) % 31 < 15);
    }
}

#[test]
fn independent_rf_reaches_transport_packets_without_allocations() {
    let mut receiver = sdrmm_channels::dvbt2::receiver::Receiver::new(None).unwrap();
    let mut packets = Vec::with_capacity(4096);
    for (bytes, expected) in [
        (
            include_bytes!("../../../fixtures/dvbt2/rf_2k_qpsk.f32").as_slice(),
            include_bytes!("../../../fixtures/dvbt2/rf_2k_qpsk.ts").as_slice(),
        ),
        (
            include_bytes!("../../../fixtures/dvbt2/rf_8k_qpsk.f32").as_slice(),
            include_bytes!("../../../fixtures/dvbt2/rf_8k_qpsk.ts").as_slice(),
        ),
        (
            include_bytes!("../../../fixtures/dvbt2/rf_8k_miso.f32").as_slice(),
            include_bytes!("../../../fixtures/dvbt2/rf_8k_miso.ts").as_slice(),
        ),
        (
            include_bytes!("../../../fixtures/dvbt2/rf_32k_qpsk.f32").as_slice(),
            include_bytes!("../../../fixtures/dvbt2/rf_32k_qpsk.ts").as_slice(),
        ),
        (
            include_bytes!("../../../fixtures/dvbt2/rf_2k_lite.f32").as_slice(),
            include_bytes!("../../../fixtures/dvbt2/rf_2k_lite.ts").as_slice(),
        ),
        (
            include_bytes!("../../../fixtures/dvbt2/rf_32k_media.f32").as_slice(),
            include_bytes!("../../../fixtures/dvbt2/rf_32k_media.ts").as_slice(),
        ),
    ] {
        let iq = reference(bytes);
        receiver.reset();
        packets.clear();
        let previous = receiver.frames;
        assert_no_alloc("DVB-T2 RF to TS", || {
            for chunk in iq.chunks(1009) {
                let before = packets.len();
                receiver.push(chunk, &mut packets);
                if packets.len() > before {
                    assert!(receiver.locked());
                }
            }
        });
        assert_eq!(receiver.frames - previous, 2, "{:?}", receiver.last_error);
        assert_eq!(receiver.errors, 0, "{:?}", receiver.last_error);
        assert_eq!(receiver.report().errors, 0, "{:?}", receiver.report());
        assert_eq!(packets.concat(), expected);
    }
}

#[test]
fn rf_acquisition_survives_noise_echoes_frequency_error_and_reacquisition() {
    let original = reference(include_bytes!("../../../fixtures/dvbt2/rf_2k_qpsk.f32"));
    let expected = include_bytes!("../../../fixtures/dvbt2/rf_2k_qpsk.ts");
    let mut receiver = sdrmm_channels::dvbt2::receiver::Receiver::new(Some(7)).unwrap();
    let mut packets = Vec::with_capacity(4096);
    for offset in [-0.018, 0.013] {
        let mut state = 0xdeadbeef_u32;
        let mut noise = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f64 / u32::MAX as f64 - 0.5) as f32 * 0.04
        };
        let iq: Vec<_> = original
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let echo = if i >= 17 {
                    original[i - 17] * Complex::new(0.18, -0.09)
                } else {
                    Complex::default()
                };
                (p + echo) * Complex::from_polar(0.7, offset * i as f32 + 1.1)
                    + Complex::new(noise(), noise())
            })
            .collect();
        receiver.reset();
        packets.clear();
        for chunk in iq.chunks(997) {
            receiver.push(chunk, &mut packets);
        }
        assert_eq!(
            packets.concat(),
            expected,
            "{:?} {:?}",
            receiver.last_error,
            receiver.report()
        );
    }
}
