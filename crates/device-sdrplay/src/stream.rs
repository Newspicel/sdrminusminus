use std::{
    cell::UnsafeCell,
    ffi::{c_int, c_uint, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use sdrmm_device::{DeviceError, RxSink, Sample, lock};

use crate::{
    api::{DevHandle, Sdrplay},
    ffi,
};

const SAMPLE_SCALE: f32 = 1.0 / 32_768.0;
const DEFAULT_BLOCK: usize = 4096;

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
}

pub struct StreamContext {
    slots: [UnsafeCell<Option<Slot>>; 2],
    state: Arc<StreamState>,
}

// Each stream callback is driven by its own tuner's API thread and touches only its own slot,
// and the monitor thread only reclaims the slots after sdrplay_api_Uninit has joined those
// threads, so the cells are never shared between threads at the same time.
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
            out: Vec::with_capacity(DEFAULT_BLOCK),
        }
    }

    fn deliver(&mut self, xi: *const i16, xq: *const i16, count: usize) {
        self.out.clear();
        self.out.reserve(count);
        for index in 0..count {
            let i = unsafe { *xi.add(index) };
            let q = unsafe { *xq.add(index) };
            self.out.push(Sample::new(
                f32::from(i) * SAMPLE_SCALE,
                f32::from(q) * SAMPLE_SCALE,
            ));
        }
        self.sink.push(&self.out);
    }
}

fn deliver(context: *mut c_void, index: usize, xi: *mut i16, xq: *mut i16, count: c_uint) {
    if context.is_null() || xi.is_null() || xq.is_null() || count == 0 {
        return;
    }
    let context = unsafe { &*context.cast::<StreamContext>() };
    let Some(cell) = context.slots.get(index) else {
        return;
    };
    let slot = unsafe { &mut *cell.get() };
    if let Some(slot) = slot.as_mut() {
        slot.deliver(xi, xq, count as usize);
    }
    context
        .state
        .samples
        .fetch_add(u64::from(count), Ordering::Relaxed);
}

unsafe extern "C" fn stream_a(
    xi: *mut i16,
    xq: *mut i16,
    _params: *mut ffi::StreamCbParamsT,
    num_samples: c_uint,
    _reset: c_uint,
    context: *mut c_void,
) {
    deliver(context, 0, xi, xq, num_samples);
}

unsafe extern "C" fn stream_b(
    xi: *mut i16,
    xq: *mut i16,
    _params: *mut ffi::StreamCbParamsT,
    num_samples: c_uint,
    _reset: c_uint,
    context: *mut c_void,
) {
    deliver(context, 1, xi, xq, num_samples);
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
                tracing::warn!("sdrplay reports an ADC overload — reduce RF gain");
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

    use super::*;
    use crate::testing::FakeApi;

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
            xi.as_mut_ptr(),
            xq.as_mut_ptr(),
            3,
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
        deliver(pointer, 1, xi.as_mut_ptr(), xq.as_mut_ptr(), 8);
        assert!(rx_a.try_recv().is_err());
        assert_eq!(rx_b.try_recv().expect("tuner b block"), 8);
        deliver(pointer, 0, xi.as_mut_ptr(), xq.as_mut_ptr(), 4);
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
            xi.as_mut_ptr(),
            xq.as_mut_ptr(),
            4,
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
        deliver(pointer, 0, xi.as_mut_ptr(), xq.as_mut_ptr(), 0);
        deliver(pointer, 0, std::ptr::null_mut(), xq.as_mut_ptr(), 4);
        deliver(std::ptr::null_mut(), 0, xi.as_mut_ptr(), xq.as_mut_ptr(), 4);
        assert!(rx.try_recv().is_err());
        assert_eq!(context.state().samples(), 0);
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
