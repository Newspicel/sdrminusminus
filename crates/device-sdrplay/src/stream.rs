use std::{
    cell::UnsafeCell,
    ffi::{c_int, c_uint, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use sdrmm_device::{
    DeviceError, GapScope, LaneMark, RxSink, Sample, UNKNOWN_ERROR, Uncertainty, lock,
};

use crate::{
    api::{DevHandle, Sdrplay},
    ffi,
};

const SAMPLE_SCALE: f32 = 1.0 / 32_768.0;
const MAX_CALLBACK: usize = 16_384;
const BACKWARDS: u32 = 1 << 31;

pub struct StreamState {
    api: Arc<dyn Sdrplay>,
    dev: DevHandle,
    samples: AtomicU64,
    master_ready: AtomicBool,
    fatal: Mutex<Option<String>>,
}

impl StreamState {
    #[must_use]
    pub fn new(api: Arc<dyn Sdrplay>, dev: DevHandle) -> Arc<Self> {
        Arc::new(Self {
            api,
            dev,
            samples: AtomicU64::new(0),
            master_ready: AtomicBool::new(false),
            fatal: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn master_ready(&self) -> bool {
        self.master_ready.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn fatal(&self) -> Option<String> {
        lock(&self.fatal).clone()
    }

    pub fn fail(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let mut fatal = lock(&self.fatal);
        if fatal.is_none() {
            tracing::warn!("sdrplay stream failed: {reason}");
            *fatal = Some(reason);
        }
    }
}

struct Slot {
    sink: RxSink,
    out: Vec<Sample>,
    expected: Option<u32>,
}

pub struct StreamContext {
    slots: [UnsafeCell<Option<Slot>>; 2],
    state: Arc<StreamState>,
}

unsafe impl Send for StreamContext {}
unsafe impl Sync for StreamContext {}

impl StreamContext {
    #[must_use]
    pub fn new(sinks: Vec<RxSink>, state: Arc<StreamState>) -> Box<Self> {
        let mut sinks = sinks.into_iter();
        let slots = [
            UnsafeCell::new(sinks.next().map(Slot::new)),
            UnsafeCell::new(sinks.next().map(Slot::new)),
        ];
        Box::new(Self { slots, state })
    }

    #[cfg(test)]
    #[must_use]
    pub fn state(&self) -> &Arc<StreamState> {
        &self.state
    }

    #[must_use]
    pub fn callbacks() -> ffi::CallbackFnsT {
        ffi::CallbackFnsT {
            stream_a: Some(stream_a),
            stream_b: Some(stream_b),
            event: Some(event),
        }
    }

    pub fn fail_sinks(&mut self, reason: &str) {
        for slot in &mut self.slots {
            if let Some(slot) = slot.get_mut() {
                slot.sink.fail(DeviceError::Io(reason.to_string()));
            }
        }
    }
}

impl Slot {
    fn new(sink: RxSink) -> Self {
        Self {
            sink,
            out: Vec::with_capacity(MAX_CALLBACK),
            expected: None,
        }
    }

    fn track(&mut self, params: &ffi::StreamCbParamsT, count: c_uint, reset: bool) {
        let first = params.first_sample_num;
        if reset || params.fs_changed != 0 {
            let cause = if reset {
                Uncertainty::Reset
            } else {
                Uncertainty::RateWrite
            };
            self.sink.realigned(cause, UNKNOWN_ERROR, GapScope::Device);
        } else if let Some(expected) = self.expected {
            match first.wrapping_sub(expected) {
                0 => {}
                gap if gap < BACKWARDS => self.sink.dropped(u64::from(gap)),
                _ => self
                    .sink
                    .realigned(Uncertainty::Reset, UNKNOWN_ERROR, GapScope::Device),
            }
        }
        if params.rf_changed != 0 {
            self.sink.mark(LaneMark::Retuned { in_flight: 0 });
        }
        self.expected = Some(first.wrapping_add(count));
    }

    fn deliver(&mut self, xi: *const i16, xq: *const i16, count: usize) {
        let mut start = 0;
        while start < count {
            let end = count.min(start + MAX_CALLBACK);
            self.out.clear();
            for index in start..end {
                let i = unsafe { *xi.add(index) };
                let q = unsafe { *xq.add(index) };
                self.out.push(Sample::new(
                    f32::from(i) * SAMPLE_SCALE,
                    f32::from(q) * SAMPLE_SCALE,
                ));
            }
            self.sink.push(&self.out);
            start = end;
        }
    }
}

struct Callback {
    xi: *const i16,
    xq: *const i16,
    params: *const ffi::StreamCbParamsT,
    count: c_uint,
    reset: c_uint,
}

fn deliver(context: *mut c_void, index: usize, callback: &Callback) {
    if context.is_null() {
        return;
    }
    let context = unsafe { &*context.cast::<StreamContext>() };
    let Some(cell) = context.slots.get(index) else {
        return;
    };
    let slot = unsafe { &mut *cell.get() };
    let Some(slot) = slot.as_mut() else {
        if !callback.xi.is_null() && !callback.xq.is_null() {
            context
                .state
                .samples
                .fetch_add(u64::from(callback.count), Ordering::Relaxed);
        }
        return;
    };
    if let Some(params) = unsafe { callback.params.as_ref() } {
        slot.track(params, callback.count, callback.reset != 0);
    }
    if callback.xi.is_null() || callback.xq.is_null() || callback.count == 0 {
        return;
    }
    slot.deliver(callback.xi, callback.xq, callback.count as usize);
    context
        .state
        .samples
        .fetch_add(u64::from(callback.count), Ordering::Relaxed);
}

unsafe extern "C" fn stream_a(
    xi: *mut i16,
    xq: *mut i16,
    params: *mut ffi::StreamCbParamsT,
    num_samples: c_uint,
    reset: c_uint,
    context: *mut c_void,
) {
    let callback = Callback {
        xi,
        xq,
        params,
        count: num_samples,
        reset,
    };
    deliver(context, 0, &callback);
}

unsafe extern "C" fn stream_b(
    xi: *mut i16,
    xq: *mut i16,
    params: *mut ffi::StreamCbParamsT,
    num_samples: c_uint,
    reset: c_uint,
    context: *mut c_void,
) {
    let callback = Callback {
        xi,
        xq,
        params,
        count: num_samples,
        reset,
    };
    deliver(context, 1, &callback);
}

unsafe extern "C" fn event(
    event_id: c_int,
    tuner: c_int,
    params: *mut ffi::EventParamsT,
    context: *mut c_void,
) {
    if context.is_null() {
        return;
    }
    let state = unsafe { &*context.cast::<StreamContext>() }.state.clone();
    match event_id {
        ffi::EVENT_POWER_OVERLOAD_CHANGE => {
            let detected = params.is_null()
                || unsafe { (*params).power_overload_params } == ffi::OVERLOAD_DETECTED;
            if let Err(error) = state.api.update(
                state.dev,
                tuner,
                ffi::UPDATE_CTRL_OVERLOAD_MSG_ACK,
                ffi::UPDATE_EXT1_NONE,
            ) {
                tracing::warn!("sdrplay overload acknowledgement failed: {error}");
            }
            if detected {
                tracing::warn!("sdrplay reports an ADC overload: reduce RF gain");
            }
        }
        ffi::EVENT_DEVICE_REMOVED => state.fail("the SDRplay receiver was unplugged"),
        ffi::EVENT_DEVICE_FAILURE => state.fail("the SDRplay receiver reported a failure"),
        ffi::EVENT_RSPDUO_MODE_CHANGE => {
            let change = if params.is_null() {
                ffi::DUO_EVENT_MASTER_INITIALISED
            } else {
                unsafe { (*params).rsp_duo_mode_params }
            };
            match change {
                ffi::DUO_EVENT_MASTER_INITIALISED => {
                    state.master_ready.store(true, Ordering::Release);
                }
                ffi::DUO_EVENT_MASTER_DLL_DISAPPEARED => {
                    state.fail("the RSPduo master application stopped");
                }
                ffi::DUO_EVENT_SLAVE_DLL_DISAPPEARED | ffi::DUO_EVENT_SLAVE_DETACHED => {
                    tracing::info!("the RSPduo slave application detached");
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use sdrmm_device::{LaneEvent, SinkItem};

    use super::*;
    use crate::testing::FakeApi;

    fn plain(xi: *mut i16, xq: *mut i16, count: c_uint) -> Callback {
        Callback {
            xi,
            xq,
            params: std::ptr::null(),
            count,
            reset: 0,
        }
    }

    fn state() -> Arc<StreamState> {
        StreamState::new(Arc::new(FakeApi::rsp1a()), DevHandle(std::ptr::null_mut()))
    }

    #[test]
    fn samples_reach_the_sink_scaled_to_unit_range() {
        let (tx, rx) = mpsc::channel();
        let mut context = StreamContext::new(
            vec![RxSink::new(move |samples, _| {
                tx.send(samples.to_vec()).unwrap()
            })],
            state(),
        );
        let mut xi = [32767_i16, -32768, 0];
        let mut xq = [0_i16, 16384, -16384];
        deliver(
            std::ptr::from_mut(context.as_mut()).cast(),
            0,
            &plain(xi.as_mut_ptr(), xq.as_mut_ptr(), 3),
        );
        let block = rx.try_recv().expect("one block");
        assert_eq!(block.len(), 3);
        assert!((block[0].re - 0.999_97).abs() < 1e-4);
        assert!((block[1].re + 1.0).abs() < 1e-6);
        assert!((block[2].im + 0.5).abs() < 1e-6);
        assert_eq!(context.state().samples(), 3);
    }

    #[test]
    fn each_tuner_delivers_only_to_its_own_sink() {
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        let mut context = StreamContext::new(
            vec![
                RxSink::new(move |samples, _| tx_a.send(samples.len()).unwrap()),
                RxSink::new(move |samples, _| tx_b.send(samples.len()).unwrap()),
            ],
            state(),
        );
        let pointer = std::ptr::from_mut(context.as_mut()).cast();
        let mut xi = [1_i16; 8];
        let mut xq = [1_i16; 8];
        deliver(pointer, 1, &plain(xi.as_mut_ptr(), xq.as_mut_ptr(), 8));
        assert!(rx_a.try_recv().is_err());
        assert_eq!(rx_b.try_recv().expect("tuner b block"), 8);
        deliver(pointer, 0, &plain(xi.as_mut_ptr(), xq.as_mut_ptr(), 4));
        assert_eq!(rx_a.try_recv().expect("tuner a block"), 4);
    }

    #[test]
    fn a_stream_with_no_sink_for_that_tuner_is_ignored() {
        let mut context = StreamContext::new(vec![RxSink::new(|_, _| {})], state());
        let mut xi = [1_i16; 4];
        let mut xq = [1_i16; 4];
        deliver(
            std::ptr::from_mut(context.as_mut()).cast(),
            1,
            &plain(xi.as_mut_ptr(), xq.as_mut_ptr(), 4),
        );
        assert_eq!(context.state().samples(), 4);
    }

    #[test]
    fn an_empty_or_null_block_is_dropped_without_a_push() {
        let (tx, rx) = mpsc::channel();
        let mut context =
            StreamContext::new(vec![RxSink::new(move |_, _| tx.send(()).unwrap())], state());
        let pointer = std::ptr::from_mut(context.as_mut()).cast();
        let mut xi = [1_i16; 4];
        let mut xq = [1_i16; 4];
        deliver(pointer, 0, &plain(xi.as_mut_ptr(), xq.as_mut_ptr(), 0));
        deliver(pointer, 0, &plain(std::ptr::null_mut(), xq.as_mut_ptr(), 4));
        deliver(
            std::ptr::null_mut(),
            0,
            &plain(xi.as_mut_ptr(), xq.as_mut_ptr(), 4),
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(context.state().samples(), 0);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Seen {
        Samples { index: u64, len: usize },
        Event(LaneEvent),
    }

    fn recorded() -> (RxSink, Arc<Mutex<Vec<Seen>>>) {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = seen.clone();
        let sink = RxSink::with_items(
            move |item: SinkItem<'_>| {
                lock(&log).push(match item {
                    SinkItem::Samples { samples, index } => Seen::Samples {
                        index,
                        len: samples.len(),
                    },
                    SinkItem::Event(event) => Seen::Event(event),
                });
            },
            |_| {},
        );
        (sink, seen)
    }

    fn params(first: u32, count: u32) -> ffi::StreamCbParamsT {
        ffi::StreamCbParamsT {
            first_sample_num: first,
            gr_changed: 0,
            rf_changed: 0,
            fs_changed: 0,
            num_samples: count,
        }
    }

    struct Tuner {
        context: Box<StreamContext>,
        seen: Arc<Mutex<Vec<Seen>>>,
        xi: Vec<i16>,
        xq: Vec<i16>,
    }

    impl Tuner {
        fn new(len: usize) -> Self {
            let (sink, seen) = recorded();
            Self {
                context: StreamContext::new(vec![sink], state()),
                seen,
                xi: vec![1; len],
                xq: vec![1; len],
            }
        }

        fn callback(&mut self, params: &ffi::StreamCbParamsT, reset: bool) {
            let callback = Callback {
                xi: self.xi.as_ptr(),
                xq: self.xq.as_ptr(),
                params,
                count: params.num_samples,
                reset: c_uint::from(reset),
            };
            deliver(
                std::ptr::from_mut(self.context.as_mut()).cast(),
                0,
                &callback,
            );
        }

        fn seen(&self) -> Vec<Seen> {
            lock(&self.seen).clone()
        }
    }

    #[test]
    fn a_skipped_first_sample_num_becomes_a_gap() {
        let mut tuner = Tuner::new(100);
        tuner.callback(&params(1_000, 100), false);
        tuner.callback(&params(1_150, 100), false);
        tuner.callback(&params(1_250, 100), false);
        assert_eq!(
            tuner.seen(),
            vec![
                Seen::Samples { index: 0, len: 100 },
                Seen::Samples {
                    index: 150,
                    len: 100
                },
                Seen::Samples {
                    index: 250,
                    len: 100
                },
            ]
        );
    }

    #[test]
    fn a_counter_that_wraps_is_not_a_gap() {
        let mut tuner = Tuner::new(100);
        tuner.callback(&params(u32::MAX - 49, 100), false);
        tuner.callback(&params(50, 100), false);
        assert_eq!(
            tuner.seen()[1],
            Seen::Samples {
                index: 100,
                len: 100
            }
        );
    }

    #[test]
    fn a_reset_marks_the_timeline_uncertain() {
        let mut tuner = Tuner::new(100);
        tuner.callback(&params(1_000, 100), false);
        tuner.callback(&params(0, 100), true);
        tuner.callback(&params(100, 100), false);
        let mut rate = params(9_000, 100);
        rate.fs_changed = 1;
        tuner.callback(&rate, false);
        tuner.callback(&params(500, 100), false);
        assert_eq!(
            tuner.seen(),
            vec![
                Seen::Samples { index: 0, len: 100 },
                Seen::Event(LaneEvent::Uncertain {
                    at: 100,
                    error: UNKNOWN_ERROR,
                    scope: GapScope::Device,
                    cause: Uncertainty::Reset,
                }),
                Seen::Samples {
                    index: 100,
                    len: 100
                },
                Seen::Samples {
                    index: 200,
                    len: 100
                },
                Seen::Event(LaneEvent::Uncertain {
                    at: 300,
                    error: UNKNOWN_ERROR,
                    scope: GapScope::Device,
                    cause: Uncertainty::RateWrite,
                }),
                Seen::Samples {
                    index: 300,
                    len: 100
                },
                Seen::Event(LaneEvent::Uncertain {
                    at: 400,
                    error: UNKNOWN_ERROR,
                    scope: GapScope::Device,
                    cause: Uncertainty::Reset,
                }),
                Seen::Samples {
                    index: 400,
                    len: 100
                },
            ],
            "a counter that went backwards is a reset nobody announced"
        );
    }

    #[test]
    fn rf_changed_marks_a_retune() {
        let mut tuner = Tuner::new(100);
        tuner.callback(&params(0, 100), false);
        let mut retuned = params(100, 100);
        retuned.rf_changed = 1;
        tuner.callback(&retuned, false);
        assert_eq!(
            tuner.seen(),
            vec![
                Seen::Samples { index: 0, len: 100 },
                Seen::Event(LaneEvent::Mark {
                    at: 100,
                    mark: LaneMark::Retuned { in_flight: 0 },
                }),
                Seen::Samples {
                    index: 100,
                    len: 100
                },
            ]
        );
    }

    #[test]
    fn a_large_callback_is_chunked_without_reserving() {
        let mut tuner = Tuner::new(40_000);
        let before = {
            let slot = unsafe { &*tuner.context.slots[0].get() };
            let slot = slot.as_ref().expect("a slot");
            (slot.out.as_ptr(), slot.out.capacity())
        };
        tuner.callback(&params(0, 40_000), false);
        assert_eq!(
            tuner.seen(),
            vec![
                Seen::Samples {
                    index: 0,
                    len: MAX_CALLBACK
                },
                Seen::Samples {
                    index: MAX_CALLBACK as u64,
                    len: MAX_CALLBACK
                },
                Seen::Samples {
                    index: 2 * MAX_CALLBACK as u64,
                    len: 40_000 - 2 * MAX_CALLBACK
                },
            ]
        );
        let slot = unsafe { &*tuner.context.slots[0].get() };
        let slot = slot.as_ref().expect("a slot");
        assert_eq!((slot.out.as_ptr(), slot.out.capacity()), before);
        assert_eq!(before.1, MAX_CALLBACK);
    }

    #[test]
    fn a_removed_device_records_the_first_reason_only() {
        let state = state();
        state.fail("first");
        state.fail("second");
        assert_eq!(state.fatal().as_deref(), Some("first"));
    }

    #[test]
    fn a_fatal_event_reaches_the_sinks_fatal_handler() {
        let (tx, rx) = mpsc::channel();
        let mut context = StreamContext::new(
            vec![RxSink::with_fatal_handler(
                |_, _| {},
                move |error| tx.send(error.to_string()).unwrap(),
            )],
            state(),
        );
        context.fail_sinks("the SDRplay receiver was unplugged");
        assert!(rx.try_recv().expect("fatal").contains("unplugged"));
    }
}
