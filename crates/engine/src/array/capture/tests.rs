use num_complex::Complex;
use rtrb::RingBuffer;
use sdrmm_wire::ArrayCalSource;

use super::*;
use crate::array::{
    align::{AlignNote, AlignNotes},
    board::StatusBoard,
    window::Windows,
};

const RATE: f64 = 48_000.0;

fn request(id: u32, start: CaptureStart, len: usize, decimation: usize) -> CaptureRequest {
    CaptureRequest {
        id,
        kind: CaptureKind::Solve,
        start,
        len,
        decimation,
        source: ArrayCalSource::Noise,
        equaliser: false,
    }
}

fn pool(lanes: usize) -> Consumer<Box<CaptureBuffers>> {
    let (mut producer, consumer) = RingBuffer::new(CAPTURE_POOL);
    for _ in 0..CAPTURE_POOL {
        let _ = producer.push(Box::new(CaptureBuffers::new(lanes, 4_096)));
    }
    consumer
}

fn context(windows: &Windows) -> FillContext<'_> {
    FillContext {
        windows,
        discontinuous: false,
        generation: 3,
        sample_rate: RATE,
        offsets: &[0, 7],
        centers_hz: &[1e6, 1e6],
        devices: &[0, 1],
    }
}

fn ramp(start: u64, len: usize) -> Vec<Complex<f32>> {
    (0..len as u64)
        .map(|at| Complex::new((start + at) as f32, 0.0))
        .collect()
}

#[test]
fn a_live_capture_decimates_into_a_job() {
    let mut free = pool(2);
    let mut slot = CaptureSlot::new(2);
    assert!(slot.arm(request(1, CaptureStart::Now, 100, 4), &mut free));
    assert!(!slot.arm(request(2, CaptureStart::Now, 100, 4), &mut free));
    let windows = Windows::new(2, RATE);
    let lane = ramp(1_000, 400);
    let mut done = None;
    for chunk in 0..4 {
        let at = chunk * 100;
        let views = [&lane[at..at + 100], &lane[at..at + 100]];
        if let Filled::Done(job) = slot.fill(&views, 1_000 + at as u64, &context(&windows)) {
            done = Some(job);
            break;
        }
    }
    let job = done.expect("capture complete");
    assert_eq!(job.request.id, 1);
    assert_eq!(job.buffers.first_index, 1_000);
    assert_eq!(job.buffers.generation, 3);
    assert_eq!(job.buffers.offsets[1], 7);
    assert_eq!(job.buffers.devices[..2], [0, 1]);
    assert_eq!(job.buffers.lanes[0].len(), 100);
    assert!((job.buffers.lanes[0][0].re - 1_001.5).abs() < 1e-3);
    assert!(!slot.armed());
}

#[test]
fn a_break_in_the_stream_refuses_the_capture() {
    let mut free = pool(1);
    let mut slot = CaptureSlot::new(1);
    assert!(slot.arm(request(9, CaptureStart::Now, 1_000, 1), &mut free));
    let windows = Windows::new(1, RATE);
    let lane = ramp(0, 100);
    assert!(matches!(
        slot.fill(&[&lane], 0, &context(&windows)),
        Filled::Waiting
    ));
    assert!(matches!(
        slot.fill(&[&lane], 500, &context(&windows)),
        Filled::Aborted(9)
    ));
    assert!(slot.arm(request(10, CaptureStart::Now, 1_000, 1), &mut free));
}

#[test]
fn a_noise_capture_waits_for_the_reference_window() {
    let mut free = pool(1);
    let mut slot = CaptureSlot::new(1);
    assert!(slot.arm(request(4, CaptureStart::NoiseWindow, 1_024, 1), &mut free));
    let mut windows = Windows::new(1, RATE);
    let board = StatusBoard::new(1);
    let frame = super::super::LiveFrame {
        sample_rate: RATE,
        center_hz: 1e6,
        lane_centers_hz: vec![1e6],
        orientation: sdrmm_wire::ArrayOrientation::default(),
        tier: sdrmm_wire::Coherence::TimeSync,
        keeps_phase: false,
        needs_time: true,
        tuning: sdrmm_wire::ArrayTuningMode::Together,
        dc_block: false,
        in_flight: 0,
        devices: [0; 16],
    };
    let quiet = vec![Complex::new(0.01, 0.0); 16_384];
    let loud = vec![Complex::new(0.3, 0.0); 16_384];
    let empty = AlignNotes::new();
    windows.observe(&empty, &[&quiet], 0, &frame, &board);
    assert!(matches!(
        slot.fill(&[&quiet], 0, &context(&windows)),
        Filled::Waiting
    ));
    let mut notes = AlignNotes::new();
    notes.push(AlignNote::Mark {
        lane: 0,
        at: 16_384 + 4_096,
        mark: sdrmm_device::LaneMark::NoiseSource {
            on: true,
            in_flight: 0,
        },
    });
    windows.observe(&notes, &[&loud], 16_384, &frame, &board);
    let (from, _) = windows.reference().expect("onset found");
    let Filled::Done(job) = slot.fill(&[&loud], 16_384, &context(&windows)) else {
        panic!("the reference window holds the capture");
    };
    assert_eq!(job.buffers.first_index, from);
    assert_eq!(job.buffers.lanes[0].len(), 1_024);
}

#[test]
fn a_request_beyond_the_buffers_is_refused() {
    let mut free = pool(1);
    let mut slot = CaptureSlot::new(1);
    assert!(!slot.arm(request(1, CaptureStart::Now, 1 << 20, 1), &mut free));
    assert!(!slot.arm(request(2, CaptureStart::Now, 10, 0), &mut free));
    assert!(slot.arm(request(3, CaptureStart::Now, 10, 1), &mut free));
}
