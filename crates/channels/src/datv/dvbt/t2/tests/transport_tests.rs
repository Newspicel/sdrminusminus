use super::*;
use crate::datv::{dvbs::PACKET, dvbs2::bb::crc8};

fn bits(bytes: &[u8]) -> Vec<bool> {
    bytes
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| byte >> i & 1 == 1))
        .collect()
}

fn frame(
    payload: &[bool],
    start: usize,
    period: usize,
    hem: bool,
    npd: bool,
    issy: usize,
) -> Vec<bool> {
    let distance = (period - start % period) % period;
    let distance = if distance >= payload.len() {
        u16::MAX
    } else {
        distance as u16
    };
    let mut header = [0; 10];
    header[0] = 0xf0 | (u8::from(npd) * 4) | (u8::from(issy > 0) * 8);
    header[2..4].copy_from_slice(
        &((if hem {
            0
        } else {
            PACKET + usize::from(npd) + issy
        }) as u16
            * 8)
        .to_be_bytes(),
    );
    header[4..6].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    header[6] = if hem { 0 } else { 0x47 };
    header[7..9].copy_from_slice(&distance.to_be_bytes());
    header[9] = crc8(&header[..9]) ^ u8::from(hem);
    let mut out = bits(&header);
    out.extend_from_slice(payload);
    out
}

fn packets() -> Vec<[u8; PACKET]> {
    (0u8..4)
        .map(|id| {
            let mut packet = [0; PACKET];
            packet[..4].copy_from_slice(&[0x47, 0x01, 0x23, 0x10 | id]);
            for (i, byte) in packet[4..].iter_mut().enumerate() {
                *byte = id.wrapping_add(i as u8);
            }
            packet
        })
        .collect()
}

fn adapted(packets: &[[u8; PACKET]], hem: bool, npd: bool, issy: usize) -> Vec<bool> {
    let mut data = Vec::new();
    let mut crc = 0;
    for packet in packets {
        if !hem {
            data.push(crc);
        }
        let at = data.len();
        data.extend_from_slice(&packet[1..]);
        data.extend(std::iter::repeat_n(0, issy));
        if npd {
            data.push(2);
        }
        crc = crc8(&data[at..]);
    }
    if !hem {
        data.push(crc);
    }
    bits(&data)
}

#[test]
fn normal_and_high_efficiency_transport_survive_every_bit_boundary() {
    let source = packets();
    for hem in [false, true] {
        let adapted = adapted(&source, hem, false, 0);
        let period = (PACKET - usize::from(hem)) * 8;
        for split in 1..=period + 8 {
            let mut decoder = transport::Transport::default();
            let mut output = Vec::with_capacity(8);
            let first = frame(&adapted[..split], 0, period, hem, false, 0);
            let second = frame(&adapted[split..], split, period, hem, false, 0);
            assert_eq!(
                decoder.push(&first, &mut output).unwrap().discontinuities,
                0
            );
            assert_eq!(
                decoder.push(&second, &mut output).unwrap().discontinuities,
                0,
                "hem={hem} split={split}"
            );
            assert_eq!(output, source, "hem={hem} split={split}");
        }
    }
}

#[test]
fn issy_and_deleted_null_packets_do_not_enter_the_payload() {
    let source = packets();
    for (hem, issy) in [(false, 0), (false, 2), (false, 3), (true, 0)] {
        let adapted = adapted(&source, hem, true, issy);
        let period = (PACKET - usize::from(hem) + 1 + issy) * 8;
        let mut decoder = transport::Transport::default();
        let mut output = Vec::with_capacity(32);
        let report = decoder
            .push(&frame(&adapted, 0, period, hem, true, issy), &mut output)
            .unwrap();
        assert_eq!(report.packets, 12);
        for (i, packet) in source.iter().enumerate() {
            assert_eq!(&output[3 * i][..4], &[0x47, 0x1f, 0xff, 0x10]);
            assert_eq!(&output[3 * i + 1][..4], &[0x47, 0x1f, 0xff, 0x10]);
            assert_eq!(&output[3 * i + 2], packet);
        }
    }
}

#[test]
fn bad_crc_header_discontinuity_and_capacity_are_observable() {
    let source = packets();
    let mut data = adapted(&source, false, false, 0);
    data[100] ^= true;
    let mut decoder = transport::Transport::default();
    let mut output = Vec::with_capacity(8);
    let report = decoder
        .push(&frame(&data, 0, PACKET * 8, false, false, 0), &mut output)
        .unwrap();
    assert_eq!(report.crc_errors, 1);
    assert_eq!(output, &source[1..]);
    output.clear();
    let clean = adapted(&source, true, false, 0);
    let mut bad_header = frame(&clean, 0, (PACKET - 1) * 8, true, false, 0);
    bad_header[30] ^= true;
    assert_eq!(
        decoder.push(&bad_header, &mut output),
        Err(DecodeError::Header)
    );
    let valid = frame(&clean, 0, (PACKET - 1) * 8, true, false, 0);
    assert_eq!(
        decoder.push(&valid, &mut Vec::new()),
        Err(DecodeError::Capacity)
    );
    decoder
        .push(
            &frame(&clean[..17], 0, (PACKET - 1) * 8, true, false, 0),
            &mut output,
        )
        .unwrap();
    assert_eq!(
        decoder.push(&valid, &mut output).unwrap().discontinuities,
        1
    );
    assert_eq!(output, source);
}
