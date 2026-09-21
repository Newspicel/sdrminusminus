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
