use super::*;
use crate::datv::{
    dvbs2::bb::crc8,
    dvbt::t2::{rf_tests, tests::encode},
};

fn payload(coding: Coding, index: usize) -> (Vec<bool>, [u8; PACKET]) {
    let mut packet = [index as u8; PACKET];
    packet[..4].copy_from_slice(&[0x47, 0x01, 0x23, 0x10 | index as u8]);
    let mut header = [0xf0, 0, 0, 0, 0x05, 0xd8, 0, 0, 0, 0];
    header[9] = crc8(&header[..9]) ^ 1;
    let mut message: Vec<_> = header
        .iter()
        .chain(&packet[1..])
        .flat_map(|&byte| (0..8).rev().map(move |bit| byte >> bit & 1 != 0))
        .collect();
    message.resize(coding.message(), false);
    (message, packet)
}

fn configuration(time_length: usize, across: bool, kind: u8) -> (Pre, Post, Plp) {
    let mut pre = rf_tests::pre(2048, 1, false);
    pre.frames = 4;
    let plp = Plp {
        id: 7,
        kind,
        payload: 3,
        first_frame: 0,
        group: 0,
        coding: Coding {
            frame: Frame::Short,
            rate: Rate::R1_2,
            constellation: Constellation::Qpsk,
            rotated: false,
            lite: false,
        },
        max_blocks: 5,
        frame_interval: 1,
        time_length,
        time_across_frames: across,
        inband_a: false,
        inband_b: false,
        mode: 2,
        start: 17,
        blocks: 5,
    };
    let mut post = Post {
        subslices: if kind == 2 { 3 } else { 1 },
        frame: 0,
        subslice_interval: 22000,
        type2_start: 17,
        fef_length: 0,
        fef_interval: 0,
        plps: [None; 256],
        count: 1,
    };
    post.plps[0] = Some(plp);
    (pre, post, plp)
}

fn transmitted(plp: Plp) -> (Vec<Complex<f32>>, Vec<[u8; PACKET]>) {
    let groups = if plp.time_across_frames {
        1
    } else {
        plp.time_length.max(1)
    };
    let mut signal = Vec::new();
    let mut expected = Vec::new();
    let mut counter = 0;
    for group in 0..groups {
        let blocks = interleave::time_block_size(plp.blocks, groups, group).unwrap();
        let mut source = Vec::new();
        for block in 0..blocks {
            let (message, packet) = payload(plp.coding, counter);
            source.extend(encode(plp.coding, &message, block));
            expected.push(packet);
            counter += 1;
        }
        if plp.time_length == 0 {
            signal.extend(source);
        } else {
            let rows = plp.coding.cells() / 5;
            for row in 0..rows {
                for column in 0..blocks * 5 {
                    signal.push(source[column * rows + row]);
                }
            }
        }
    }
    (signal, expected)
}

#[test]
fn type1_type2_and_interframe_schedules_recover_only_selected_cells() {
    let mut scheduler = Scheduler::new().unwrap();
    for (length, across, kind) in [
        (0, false, 1),
        (1, false, 1),
        (3, false, 2),
        (8, false, 2),
        (2, true, 2),
    ] {
        scheduler.reset();
        let (pre, mut post, plp) = configuration(length, across, kind);
        let (signal, expected) = transmitted(plp);
        let frames = plp.interleaving_frames();
        let mut packets = Vec::with_capacity(1024);
        for frame in 0..frames {
            post.frame = frame;
            scheduler.begin(pre, &post, Some(7)).unwrap();
            let per_frame = signal.len() / frames;
            let per_slice = per_frame / post.subslices;
            let mut cells =
                vec![
                    Complex::new(99.0, 99.0);
                    plp.start + (post.subslices - 1) * post.subslice_interval + per_slice + 37
                ];
            for slice in 0..post.subslices {
                let from = frame * per_frame + slice * per_slice;
                let start = plp.start + slice * post.subslice_interval;
                cells[start..start + per_slice].copy_from_slice(&signal[from..from + per_slice]);
            }
            for (chunk, data) in cells.chunks(137).enumerate() {
                scheduler
                    .push(chunk * 137, data, 0.01, &mut packets)
                    .unwrap();
            }
            scheduler.end().unwrap();
        }
        assert_eq!(
            packets,
            expected,
            "length={length} across={across} kind={kind} {:?}",
            scheduler.report()
        );
    }
}

#[test]
fn missing_slices_and_missing_interleaving_frames_are_reported() {
    let mut scheduler = Scheduler::new().unwrap();
    let (pre, mut post, _) = configuration(2, true, 2);
    post.frame = 1;
    assert_eq!(
        scheduler.begin(pre, &post, None),
        Err(DecodeError::Discontinuity)
    );
    post.frame = 0;
    scheduler.begin(pre, &post, None).unwrap();
    assert_eq!(scheduler.end(), Err(DecodeError::Discontinuity));
    assert_eq!(scheduler.begin(pre, &post, Some(99)), Err(DecodeError::Plp));
}
