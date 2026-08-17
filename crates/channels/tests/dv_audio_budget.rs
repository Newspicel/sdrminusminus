#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs, ChannelRx, DmrChannel};
use sdrmm_wire::{ChannelParams, ChannelSettings, DmrParams};

const AIR_RATE_HZ: f64 = 48_000.0;

const MAX_REAL_TIME_FRACTION: f64 = 0.5;
const MAX_ALLOCS_PER_SECOND: f64 = 200.0;
const MAX_ALLOC_BYTES_PER_SECOND: f64 = 65_536.0;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

struct Counting;

// SAFETY: every call forwards to the system allocator unchanged; the counters are a side effect
// and the returned pointer is whatever the system produced.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn recorded_call() -> Vec<Complex<f32>> {
    const DATA: &[u8] = include_bytes!("../../../fixtures/dmr_call_48k.sigmf-data");
    DATA.as_chunks::<8>()
        .0
        .iter()
        .map(|s| {
            Complex::new(
                f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            )
        })
        .collect()
}

fn channel() -> DmrChannel {
    DmrChannel::new(
        ChannelCtx {
            input_rate: AIR_RATE_HZ,
        },
        ChannelSettings {
            params: ChannelParams::Dmr(DmrParams::default()),
            ..ChannelSettings::default_for("dmr").unwrap()
        },
    )
    .unwrap()
}

/// The only test in this binary: the counting allocator is global, so a second test running
/// alongside it would charge its own allocations to this budget.
#[test]
fn a_decoded_call_stays_inside_its_real_time_and_allocation_budget() {
    let iq = recorded_call();
    let seconds = iq.len() as f64 / AIR_RATE_HZ;
    let mut chan = channel();
    let mut out = ChannelOutputs::default();

    for block in iq.chunks(4_096) {
        out.reset();
        chan.process(block, &mut out);
    }

    COUNTING.store(true, Ordering::Relaxed);
    let started = Instant::now();
    let mut audio = 0usize;
    for block in iq.chunks(4_096) {
        out.reset();
        chan.process(block, &mut out);
        audio += out.audio_pcm.len();
    }
    let elapsed = started.elapsed().as_secs_f64();
    COUNTING.store(false, Ordering::Relaxed);

    assert!(audio > 0, "the recorded call decoded to no audio at all");

    let fraction = elapsed / seconds;
    assert!(
        fraction < MAX_REAL_TIME_FRACTION,
        "decoding {seconds:.2} s of air took {:.1} ms, {fraction:.4} of real time",
        elapsed * 1e3
    );

    let allocs = ALLOCS.load(Ordering::Relaxed) as f64 / seconds;
    let bytes = BYTES.load(Ordering::Relaxed) as f64 / seconds;
    assert!(
        allocs <= MAX_ALLOCS_PER_SECOND,
        "{allocs:.0} allocations per second of air, budget {MAX_ALLOCS_PER_SECOND:.0}; \
         the steady-state voice path should reuse its buffers"
    );
    assert!(
        bytes <= MAX_ALLOC_BYTES_PER_SECOND,
        "{bytes:.0} allocated bytes per second of air, budget {MAX_ALLOC_BYTES_PER_SECOND:.0}; \
         a filter bank or table is being rebuilt instead of reset"
    );
}
