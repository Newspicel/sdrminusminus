use super::*;

fn fec_encode(
    fec: &Fec,
    message: &[bool],
    shorten: &[usize],
    puncture: &[usize],
    length: usize,
) -> Vec<bool> {
    let size = fec.information - 168;
    let mut omitted = vec![false; 16200];
    shortening(&mut omitted[..size], message.len(), shorten).unwrap();
    let mut word = vec![false; size];
    let mut bits = message.iter();
    for i in 0..size {
        if !omitted[i] {
            word[i] = *bits.next().unwrap();
        }
    }
    let mut bch = Vec::new();
    fec.bch.encode(&word, &mut bch);
    let mut encoded = Vec::new();
    fec.ldpc.encode(&bch, &mut encoded);
    let count = message.len() + 16200 - size - length;
    for i in 0..count {
        omitted[fec.information + puncture[i / 360] + puncture.len() * (i % 360)] = true;
    }
    encoded
        .into_iter()
        .zip(omitted)
        .filter_map(|(bit, omit)| (!omit).then_some(bit))
        .collect()
}

#[test]
fn shortened_punctured_codes_correct_damaged_signalling() {
    let mut decoder = Signalling::new().unwrap();
    for (is_pre, table, size, length) in [
        (true, 0, 200, 1840),
        (false, 0, 350, 1528),
        (false, 1, 350, 1528),
        (false, 2, 350, 1528),
        (false, 0, 6500, 15036),
        (false, 1, 6500, 15036),
        (false, 2, 6500, 15036),
        (false, 0, 7032, 16192),
    ] {
        let message: Vec<_> = (0..size).map(|i| (i * 13 + i / 11) % 23 < 12).collect();
        let (fec, shortened, punctured) = if is_pre {
            (&mut decoder.pre, &PRE_SHORTEN[..], &PRE_PUNCTURE[..])
        } else {
            (
                &mut decoder.post,
                &POST_SHORTEN[table][..],
                &POST_PUNCTURE[table][..],
            )
        };
        let encoded = fec_encode(fec, &message, shortened, punctured, length);
        assert_eq!(encoded.len(), length);
        let mut soft: Vec<_> = encoded
            .iter()
            .map(|&bit| if bit { -8.0 } else { 8.0 })
            .collect();
        for i in (37..length).step_by(701) {
            soft[i] *= -0.25;
        }
        let mut output = vec![false; size];
        Signalling::decode(
            fec,
            &soft,
            &mut decoder.llrs,
            &mut decoder.omitted,
            &mut decoder.word,
            &mut output,
            shortened,
            punctured,
        )
        .unwrap_or_else(|e| {
            panic!("pre={is_pre} table={table} size={size} length={length}: {e:?}")
        });
        assert_eq!(output, message);
    }
}

#[test]
fn shortening_keeps_exact_information_length() {
    for size in [1, 192, 200, 359, 360, 361, 7032] {
        for order in POST_SHORTEN {
            let mut omitted = vec![false; 7032];
            shortening(&mut omitted, size, &order).unwrap();
            assert_eq!(omitted.iter().filter(|&&v| !v).count(), size);
        }
    }
}

#[test]
fn crc_and_signalling_reject_corruption() {
    assert_eq!(crc(&[]), u32::MAX);
    let mut bits: Vec<_> = (0..168).map(|i| i % 7 == 0).collect();
    let check = crc(&bits);
    bits.extend((0..32).rev().map(|i| check >> i & 1 != 0));
    assert_eq!(crc(&bits), 0);
    bits[18] ^= true;
    assert_ne!(crc(&bits), 0);
    assert!(Pre::parse(&bits, Preamble { s1: 0, s2: 0 }).is_err());
}

#[test]
fn all_l1_constellations_reverse_the_column_interleaver_and_demultiplexer() {
    let mut decoder = Signalling::new().unwrap();
    for bits in [1, 2, 4, 6] {
        let columns = if bits > 2 { bits * 2 } else { 1 };
        let rows = 64;
        let original: Vec<_> = (0..rows * columns)
            .map(|i| (i * 71 + i / 11) % 17 < 8)
            .collect();
        let mut serial = vec![false; original.len()];
        if bits > 2 {
            let demux = if bits == 4 {
                &DEMUX16[..]
            } else {
                &DEMUX64[..]
            };
            for column in 0..columns {
                for row in 0..rows {
                    serial[row * columns + demux[column]] = original[column * rows + row];
                }
            }
        } else {
            serial.copy_from_slice(&original);
        }
        let mut cells = Vec::new();
        for chunk in serial.chunks(bits) {
            let word = chunk.iter().fold(0, |v, &b| v * 2 + usize::from(b));
            cells.push(if bits == 1 {
                Complex::new(if word == 0 { 1.0 } else { -1.0 }, 0.0)
            } else {
                bicm::point(
                    word,
                    match bits {
                        2 => Constellation::Qpsk,
                        4 => Constellation::Qam16,
                        _ => Constellation::Qam64,
                    },
                )
            });
        }
        decoder.demap(&cells, bits).unwrap();
        for (i, &bit) in original.iter().enumerate() {
            assert_eq!(decoder.demapped[i] < 0.0, bit, "{bits}: {i}");
        }
    }
}
