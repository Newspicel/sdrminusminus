use super::*;

#[test]
fn array_forwarding_keeps_buffered_samples_before_a_later_overflow_gap() {
    let (mut producer, mut consumer) = capture_ring(8);
    let (commands, command_rx) = mpsc::channel();
    let (received, output) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let waker = Arc::new(Waker::default());
    let shared = LaneShared {
        meta: Arc::new(ArcSwap::from_pointee(DspMeta {
            center_hz: 100e6,
            sample_rate: 48_000.0,
            lo_offset_hz: 0.0,
            dc_block: false,
        })),
        stop: stop.clone(),
        stalled_us: Arc::new(AtomicU64::new(0)),
        waker: waker.clone(),
    };
    let mut first = true;
    commands
        .send(DspCommand::ConnectArray {
            id: 1,
            sink: RxSink::new(move |samples, index| {
                if samples.is_empty() {
                    return;
                }
                received
                    .send((index, samples.to_vec()))
                    .expect("receive array block");
                if first {
                    first = false;
                    blocked
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release DSP");
                }
            }),
        })
        .expect("connect array");
    assert_eq!(producer.push(&[Complex::new(0.0, 0.0); 2], 0), 2);
    let (spectrum, _) = broadcast::channel(8);
    let publisher = SpectrumPublisher::new(spectrum, FFT_SIZE).expect("publisher");
    let worker = std::thread::spawn(move || {
        shared.waker.adopt_current();
        dsp_loop(
            &mut consumer,
            &command_rx,
            &shared,
            SpectrumPlan::new(FFT_SIZE, 1).analyzer(),
            publisher,
        );
    });
    let initial = output
        .recv_timeout(Duration::from_secs(5))
        .expect("DSP is blocked");
    let input: Vec<_> = (2..12)
        .map(|index| Complex::new(index as f32, 0.0))
        .collect();
    let count = producer.push(&input, 2);
    release.send(()).expect("resume DSP");
    waker.wake();
    let buffered = output
        .recv_timeout(Duration::from_secs(5))
        .expect("buffered array samples");
    let following = [Complex::new(12.0, 0.0), Complex::new(13.0, 0.0)];
    assert_eq!(producer.push(&following, 12), following.len());
    waker.wake();
    let after_gap = output
        .recv_timeout(Duration::from_secs(5))
        .expect("samples after the gap");
    stop.store(true, Ordering::Release);
    waker.wake();
    worker.join().expect("DSP exits");
    assert_eq!(initial.0, 0);
    assert_eq!(count, 6);
    assert_eq!(buffered.1, input[..count]);
    assert_eq!(
        buffered.0, 2,
        "the dropped tail must not shift the buffered prefix"
    );
    assert_eq!(after_gap, (12, following.to_vec()));
}
