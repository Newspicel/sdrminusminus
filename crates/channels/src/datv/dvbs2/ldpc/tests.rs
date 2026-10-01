use sdrmm_modem_test_support::ber::rng::Rng;

use super::{flooding::Flooding, lanes::Kernel, lanes::STRIDE, *};

const RATES: [Rate; 11] = [
    Rate::R1_4,
    Rate::R1_3,
    Rate::R2_5,
    Rate::R1_2,
    Rate::R3_5,
    Rate::R2_3,
    Rate::R3_4,
    Rate::R4_5,
    Rate::R5_6,
    Rate::R8_9,
    Rate::R9_10,
];

const EVERY_RATE: [Rate; 42] = [
    Rate::R100_180,
    Rate::R104_180,
    Rate::R116_180,
    Rate::R11_20,
    Rate::R124_180,
    Rate::R128_180,
    Rate::R132_180,
    Rate::R135_180,
    Rate::R13_18,
    Rate::R13_45,
    Rate::R140_180,
    Rate::R14_45,
    Rate::R154_180,
    Rate::R18_30,
    Rate::R20_30,
    Rate::R22_30,
    Rate::R23_36,
    Rate::R25_36,
    Rate::R26_45,
    Rate::R28_45,
    Rate::R32_45,
    Rate::R7_15,
    Rate::R7_9,
    Rate::R8_15,
    Rate::R90_180,
    Rate::R96_180,
    Rate::R9_20,
    Rate::R1_5,
    Rate::R2_9,
    Rate::R11_45,
    Rate::R1_4,
    Rate::R4_15,
    Rate::R1_3,
    Rate::R2_5,
    Rate::R1_2,
    Rate::R3_5,
    Rate::R2_3,
    Rate::R3_4,
    Rate::R4_5,
    Rate::R5_6,
    Rate::R8_9,
    Rate::R9_10,
];

fn every_code() -> impl Iterator<Item = (Rate, Frame, &'static [&'static [u16]])> {
    EVERY_RATE.into_iter().flat_map(|rate| {
        [Frame::Short, Frame::Medium, Frame::Normal]
            .into_iter()
            .filter_map(move |frame| Some((rate, frame, rate.addresses(frame)?)))
    })
}

fn message(len: usize, seed: u32) -> Vec<bool> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state & 1 == 1
        })
        .collect()
}

fn llrs(codeword: &[bool], confidence: f32) -> Vec<f32> {
    codeword
        .iter()
        .map(|&bit| if bit { -confidence } else { confidence })
        .collect()
}

fn awgn(codeword: &[bool], rate: f64, eb_n0_db: f64, rng: &mut Rng) -> Vec<f32> {
    let variance = 1.0 / (2.0 * rate * 10f64.powf(eb_n0_db / 10.0));
    codeword
        .iter()
        .map(|&bit| {
            let symbol = if bit { -1.0 } else { 1.0 };
            (2.0 * (symbol + variance.sqrt() * rng.normal()) / variance) as f32
        })
        .collect()
}

fn reference(rate: Rate, frame: Frame) -> Flooding {
    Flooding::new(frame.length(), rate.addresses(frame).expect("a table"))
}

fn kernels() -> Vec<Kernel> {
    let mut kernels = vec![Kernel::Scalar, Kernel::Vector];
    if !kernels.contains(&Kernel::detect()) {
        kernels.push(Kernel::detect());
    }
    kernels
}

#[test]
fn invalid_parity_address_tables_are_rejected() {
    assert!(Ldpc::with_addresses(Frame::Short, &[]).is_none());
    assert!(Ldpc::with_addresses(Frame::Short, &[&[]]).is_none());
    assert!(Ldpc::with_addresses(Frame::Short, &[&[SHORT as u16]]).is_none());
    assert!(Ldpc::with_addresses(Frame::Short, &[&[0][..]; 45]).is_none());
}

#[test]
fn every_code_has_the_length_its_rate_promises() {
    for (rate, information) in RATES.into_iter().zip([
        16_200, 21_600, 25_920, 32_400, 38_880, 43_200, 48_600, 51_840, 54_000, 57_600, 58_320,
    ]) {
        let code = Ldpc::new(rate, Frame::Normal).unwrap_or_else(|| panic!("{rate:?} normal"));
        assert_eq!(code.length, NORMAL);
        assert_eq!(code.information, information, "{rate:?}");
        assert_eq!(rate.information(Frame::Normal), information, "{rate:?}");
        assert!(code.parity().is_multiple_of(GROUP));
    }
    for (rate, information) in RATES[..10].iter().zip([
        3_240, 5_400, 6_480, 7_200, 9_720, 10_800, 11_880, 12_600, 13_320, 14_400,
    ]) {
        let code = Ldpc::new(*rate, Frame::Short).unwrap_or_else(|| panic!("{rate:?} short"));
        assert_eq!(code.length, SHORT);
        assert_eq!(code.information, information, "{rate:?}");
        assert_eq!(rate.information(Frame::Short), information, "{rate:?}");
    }
    assert!(Ldpc::new(Rate::R9_10, Frame::Short).is_none());
    assert_eq!(Rate::R9_10.information(Frame::Short), 0);
}

#[test]
fn every_layout_matches_the_parity_check_matrix() {
    for (rate, frame, addresses) in every_code() {
        let code = Ldpc::with_addresses(frame, addresses).expect("a code");
        let layout = &code.decoder.layout;
        let mut layered = Vec::new();
        for layer in 0..layout.layers {
            for edge in layout.layer(layer) {
                for lane in 0..GROUP {
                    if let Some(position) = edge.position(lane) {
                        layered.push((layout.check(layer, lane), position));
                    }
                }
            }
        }
        let mut flooding = Flooding::new(frame.length(), addresses).edges();
        for edge in &mut flooding {
            edge.1 = layout.position(edge.1);
        }
        layered.sort_unstable();
        flooding.sort_unstable();
        assert_eq!(layered, flooding, "{rate:?} {frame:?}");
    }
}

#[test]
fn every_encoded_word_satisfies_its_parity_checks() {
    for (rate, frame, addresses) in every_code() {
        let code = Ldpc::with_addresses(frame, addresses).expect("a code");
        let information = message(code.information, 7);
        let mut codeword = Vec::new();
        code.encode(&information, &mut codeword);
        assert_eq!(codeword.len(), code.length);
        assert!(
            reference(rate, frame).satisfies(&codeword),
            "{rate:?} {frame:?} leaves a non-zero syndrome"
        );
    }
}

#[test]
fn every_very_low_rate_code_has_the_length_its_table_promises() {
    for (rate, frame, information) in [
        (Rate::R2_9, Frame::Normal, 14_400),
        (Rate::R1_5, Frame::Medium, 6_480),
        (Rate::R11_45, Frame::Medium, 7_920),
        (Rate::R1_3, Frame::Medium, 10_800),
        (Rate::R11_45, Frame::Short, 3_960),
        (Rate::R4_15, Frame::Short, 4_320),
    ] {
        let code = Ldpc::new(rate, frame).unwrap_or_else(|| panic!("{rate:?} {frame:?}"));
        assert_eq!(code.length, frame.length(), "{rate:?} {frame:?}");
        assert_eq!(code.information, information, "{rate:?} {frame:?}");
        assert!(code.parity().is_multiple_of(GROUP), "{rate:?} {frame:?}");
        let message = message(information, 29);
        let mut codeword = Vec::new();
        code.encode(&message, &mut codeword);
        assert!(
            reference(rate, frame).satisfies(&codeword),
            "{rate:?} {frame:?}"
        );
    }
    assert!(Ldpc::new(Rate::R2_9, Frame::Short).is_none());
    assert!(Ldpc::new(Rate::R3_4, Frame::Medium).is_none());
}

#[test]
fn a_shortened_and_punctured_word_decodes_back_to_its_message() {
    for (rate, frame, shape) in [
        (
            Rate::R2_9,
            Frame::Normal,
            Shape {
                shorten: 0,
                period: 15,
                punctured: 3_240,
            },
        ),
        (
            Rate::R1_5,
            Frame::Medium,
            Shape {
                shorten: 640,
                period: 25,
                punctured: 980,
            },
        ),
        (
            Rate::R1_4,
            Frame::Short,
            Shape {
                shorten: 560,
                period: 30,
                punctured: 250,
            },
        ),
        (
            Rate::R4_15,
            Frame::Short,
            Shape {
                shorten: 0,
                period: 8,
                punctured: 1_224,
            },
        ),
    ] {
        let mut code = Ldpc::new(rate, frame).expect("a code");
        let information = message(code.message(shape), 31);
        let mut codeword = Vec::new();
        code.encode_shaped(&information, shape, &mut codeword);
        assert_eq!(
            codeword.len(),
            code.transmitted(shape),
            "{rate:?} {frame:?}"
        );
        let mut received = llrs(&codeword, 4.0);
        for position in (0..received.len()).step_by(419) {
            received[position] = -received[position];
        }
        let mut expanded = Vec::new();
        code.expand(&received, shape, &mut expanded);
        let mut out = Vec::new();
        assert!(
            code.decode(&expanded, &mut out).is_some(),
            "{rate:?} {frame:?} did not converge"
        );
        assert_eq!(out[shape.shorten..], information, "{rate:?} {frame:?}");
        assert!(out[..shape.shorten].iter().all(|&bit| !bit));
    }
}

#[test]
fn a_clean_codeword_decodes_without_an_iteration() {
    let mut code = Ldpc::new(Rate::R1_2, Frame::Short).expect("short 1/2");
    let information = message(code.information, 11);
    let mut codeword = Vec::new();
    code.encode(&information, &mut codeword);
    let mut out = Vec::new();
    assert_eq!(code.decode(&llrs(&codeword, 4.0), &mut out), Some(0));
    assert_eq!(out, information);
}

#[test]
fn scattered_errors_are_repaired() {
    let mut code = Ldpc::new(Rate::R3_4, Frame::Short).expect("short 3/4");
    let information = message(code.information, 13);
    let mut codeword = Vec::new();
    code.encode(&information, &mut codeword);
    let mut received = llrs(&codeword, 4.0);
    for position in (0..received.len()).step_by(37) {
        received[position] = -received[position];
    }
    let mut out = Vec::new();
    let iterations = code.decode(&received, &mut out).expect("a decoded frame");
    assert!(iterations > 0, "the errors were not actually present");
    assert_eq!(out, information);
}

#[test]
fn layered_decoding_repairs_errors_at_every_rate() {
    for (rate, frame, addresses) in every_code() {
        let mut code = Ldpc::with_addresses(frame, addresses).expect("a code");
        let information = message(code.information, 17);
        let mut codeword = Vec::new();
        code.encode(&information, &mut codeword);
        let mut received = llrs(&codeword, 4.0);
        for position in (0..received.len()).step_by(211) {
            received[position] = -received[position];
        }
        let mut out = Vec::new();
        let iterations = code.decode(&received, &mut out);
        assert!(
            iterations.is_some_and(|count| count > 0),
            "{rate:?} {frame:?} did not converge"
        );
        assert_eq!(out, information, "{rate:?} {frame:?}");
    }
}

#[test]
fn every_kernel_updates_a_layer_bit_exactly() {
    let mut rng = Rng::new(0x51d);
    for degree in [3, 7, 11, 30] {
        let rows = degree * STRIDE;
        let mut sample = || match rng.next_u64() % 16 {
            0 => i16::MIN,
            1 => i16::MAX,
            2 => 0,
            3 => -1,
            _ => rng.next_u64() as i16,
        };
        let gathered: Vec<i16> = (0..rows).map(|_| sample()).collect();
        let messages: Vec<i16> = (0..rows).map(|_| sample()).collect();
        let outcomes: Vec<_> = kernels()
            .into_iter()
            .map(|kernel| {
                let mut gathered = gathered.clone();
                let mut extrinsic = vec![0; rows];
                let mut messages = messages.clone();
                let unsatisfied = kernel.update(&mut gathered, &mut extrinsic, &mut messages);
                (unsatisfied, gathered, extrinsic, messages)
            })
            .collect();
        for outcome in &outcomes[1..] {
            assert!(*outcome == outcomes[0], "degree {degree}");
        }
    }
}

#[test]
fn every_kernel_decodes_a_noisy_word_bit_exactly() {
    let mut rng = Rng::new(0xfec);
    let mut code = Ldpc::new(Rate::R1_2, Frame::Normal).expect("normal 1/2");
    let information = message(code.information, 19);
    let mut codeword = Vec::new();
    code.encode(&information, &mut codeword);
    let received = awgn(&codeword, 0.5, 2.5, &mut rng);
    assert!(
        received
            .iter()
            .zip(&codeword)
            .any(|(&llr, &bit)| (llr < 0.0) != bit)
    );
    let outcomes: Vec<_> = kernels()
        .into_iter()
        .map(|kernel| {
            code.decoder.kernel = kernel;
            let mut out = Vec::new();
            let iterations = code.decode(&received, &mut out);
            (iterations, out, code.decoder.totals.clone())
        })
        .collect();
    assert!(outcomes[0].0.is_some_and(|count| count > 0));
    assert_eq!(outcomes[0].1, information);
    for outcome in &outcomes[1..] {
        assert!(*outcome == outcomes[0]);
    }
}

#[test]
fn noise_does_not_converge_on_a_codeword() {
    let mut code = Ldpc::new(Rate::R1_2, Frame::Short).expect("short 1/2");
    let mut state = 0x1357_9bdfu32;
    let received: Vec<f32> = (0..code.length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 16) as f32 / 16_384.0 - 2.0
        })
        .collect();
    let mut out = Vec::new();
    assert!(code.decode(&received, &mut out).is_none());
    assert!(out.is_empty());
    assert_eq!(code.hard_information().len(), code.information);
}

struct Trial {
    failures: usize,
    iterations: usize,
}

fn trial(rate: Rate, fraction: f64, eb_n0_db: f64) -> (Trial, Trial) {
    let mut code = Ldpc::new(rate, Frame::Short).expect("a short code");
    let mut flooding = reference(rate, Frame::Short);
    let mut rng = Rng::new(7);
    let mut old = Trial {
        failures: 0,
        iterations: 0,
    };
    let mut new = Trial {
        failures: 0,
        iterations: 0,
    };
    for seed in 0..60 {
        let information = message(code.information, seed * 7 + 3);
        let mut codeword = Vec::new();
        code.encode(&information, &mut codeword);
        let received = awgn(&codeword, fraction, eb_n0_db, &mut rng);
        match flooding.decode(&received, 30) {
            Some((iterations, _)) => old.iterations += iterations,
            None => old.failures += 1,
        }
        let mut out = Vec::new();
        match code.decode(&received, &mut out) {
            Some(iterations) => {
                assert_eq!(out, information);
                new.iterations += iterations;
            }
            None => new.failures += 1,
        }
    }
    (old, new)
}

#[test]
fn layered_fails_no_more_often_than_thirty_flooding_iterations() {
    for (rate, fraction, eb_n0_db) in [(Rate::R1_2, 0.5, 0.9), (Rate::R3_4, 0.75, 2.0)] {
        let (old, new) = trial(rate, fraction, eb_n0_db);
        assert!(old.failures > 0, "{rate:?} is not at its waterfall");
        assert!(
            new.failures <= old.failures,
            "{rate:?}: layered {} flooding {}",
            new.failures,
            old.failures
        );
    }
}

#[test]
fn layered_converges_in_far_fewer_iterations() {
    let (old, new) = trial(Rate::R3_4, 0.75, 2.7);
    assert_eq!((old.failures, new.failures), (0, 0));
    assert!(
        new.iterations * 10 <= old.iterations * 7,
        "layered {} flooding {}",
        new.iterations,
        old.iterations
    );
}
