use super::*;

fn packet(pid: u16, id: u8) -> [u8; PACKET] {
    let mut packet = [id; PACKET];
    packet[..4].copy_from_slice(&[0x47, (pid >> 8) as u8, pid as u8, 0x10]);
    packet
}

#[test]
fn common_packets_replace_only_cotimed_nulls_across_timestamp_wrap() {
    for presentation in [false, true] {
        let mut merge = Merge::default();
        let mut output = Vec::with_capacity(20);
        let mut data = Vec::new();
        let mut common = Vec::new();
        for i in 0..10 {
            let clock = if presentation && i == 0 {
                Some(Clock::Presentation(50000))
            } else {
                Some(Clock::Counter {
                    value: (32700 + 100 * i) % 32768,
                    bits: 15,
                })
            };
            data.push(TimedPacket {
                data: packet(if i % 2 == 0 { 0x1fff } else { 0x123 }, i as u8),
                clock,
            });
        }
        for i in 2..10 {
            let clock = if presentation && i == 2 {
                Some(Clock::Presentation(50200))
            } else {
                Some(Clock::Counter {
                    value: (32700 + 100 * i) % 32768,
                    bits: 15,
                })
            };
            common.push(TimedPacket {
                data: packet(0x12, i as u8),
                clock,
            });
        }
        merge.append(0, &data).unwrap();
        merge.drain(&mut output).unwrap();
        assert!(output.is_empty());
        merge.append(1, &common).unwrap();
        merge.drain(&mut output).unwrap();
        assert_eq!(output.len(), 8);
        for (i, packet) in output.iter().enumerate() {
            assert_eq!(packet[2], if i % 2 == 0 { 0x12 } else { 0x23 });
            assert_eq!(packet[4], i as u8 + 2);
        }
    }
}

#[test]
fn issy_timestamp_and_tto_fields_decode_at_the_frame_origin() {
    assert_eq!(
        Clock::parse(&[0x7f, 0xfe], 0),
        Some(Clock::Counter {
            value: 32766,
            bits: 15
        })
    );
    assert_eq!(
        Clock::parse(&[0xbf, 0xff, 0xfe], 0),
        Some(Clock::Counter {
            value: 4194302,
            bits: 22
        })
    );
    assert_eq!(
        Clock::parse(&[0xd1, 0x04, 0x80], 100),
        Some(Clock::Presentation(118))
    );
    assert_eq!(Clock::parse(&[0xc0, 0, 0], 0), None);
    assert_eq!(Clock::parse(&[0xe0, 0, 0], 0), None);
}

#[test]
fn both_plps_pass_fec_timing_and_common_packet_reconstruction() {
    use crate::datv::{
        dvbs2::bb::crc8,
        dvbt::t2::{Coding, Constellation, Frame, Rate, rf_tests, tests::encode},
    };
    let coding = Coding {
        frame: Frame::Short,
        rate: Rate::R1_2,
        constellation: Constellation::Qpsk,
        rotated: false,
        lite: false,
    };
    let mut pre = rf_tests::pre(2048, 1, false);
    pre.frames = 2;
    let plp = Plp {
        id: 3,
        kind: 0,
        payload: 3,
        first_frame: 0,
        group: 4,
        coding,
        max_blocks: 5,
        frame_interval: 1,
        time_length: 1,
        time_across_frames: false,
        inband_a: false,
        inband_b: false,
        mode: 2,
        start: 0,
        blocks: 5,
    };
    let mut data = plp;
    data.id = 7;
    data.kind = 1;
    data.start = 40500;
    let mut post = Post {
        subslices: 1,
        frame: 0,
        subslice_interval: 0,
        type2_start: 0,
        fef_length: 0,
        fef_interval: 0,
        plps: [None; 256],
        count: 2,
    };
    post.plps[..2].copy_from_slice(&[Some(plp), Some(data)]);
    let mut signal = Vec::new();
    for common in [true, false] {
        let mut coded = Vec::new();
        for block in 0..5 {
            let pid = if common {
                0x12
            } else if block % 2 == 0 {
                0x1fff
            } else {
                0x123
            };
            let packet = packet(pid, block as u8);
            let time = (1000 + block * 100) as u32;
            let mut header = [
                0xf8,
                0,
                0x80 | (time >> 16) as u8,
                (time >> 8) as u8,
                0x05,
                0xd8,
                time as u8,
                0,
                0,
                0,
            ];
            header[9] = crc8(&header[..9]) ^ 1;
            let mut word: Vec<_> = header
                .iter()
                .chain(&packet[1..])
                .flat_map(|&byte| (0..8).rev().map(move |i| byte >> i & 1 != 0))
                .collect();
            word.resize(coding.message(), false);
            coded.extend(encode(coding, &word, block));
        }
        for row in 0..1620 {
            for column in 0..25 {
                signal.push(coded[column * 1620 + row]);
            }
        }
    }
    let mut receiver = Multiplex::new().unwrap();
    let mut output = Vec::with_capacity(20);
    receiver.begin(pre, &post, Some(7), 0).unwrap();
    for (i, chunk) in signal.chunks(997).enumerate() {
        receiver.push(i * 997, chunk, 0.01, &mut output).unwrap();
    }
    receiver.end().unwrap();
    assert_eq!(receiver.report().errors, 0);
    assert_eq!(output.len(), 5);
    for (i, p) in output.iter().enumerate() {
        assert_eq!(*p, packet(if i % 2 == 0 { 0x12 } else { 0x123 }, i as u8));
    }
}
