use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Instant,
};

thread_local! {
    static THREAD_ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn count_one() {
    let _ = THREAD_ALLOCS.try_with(|count| count.set(count.get() + 1));
}

fn thread_allocs() -> u64 {
    THREAD_ALLOCS.try_with(Cell::get).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CountingAlloc;

impl CountingAlloc {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_one();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_one();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_one();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[must_use]
pub fn measure_allocs(f: impl FnOnce()) -> u64 {
    let canary = thread_allocs();
    std::hint::black_box(Box::new(0u8));
    assert!(
        thread_allocs() > canary,
        "CountingAlloc is not this binary's #[global_allocator]; a zero-alloc assertion \
         would pass vacuously"
    );
    let before = thread_allocs();
    f();
    thread_allocs() - before
}

pub fn assert_no_alloc(label: &str, f: impl FnOnce()) {
    let allocs = measure_allocs(f);
    assert_eq!(
        allocs, 0,
        "{label}: {allocs} allocation(s) on the steady-state path"
    );
}

pub const TIMED_BATCHES: usize = 5;

#[must_use]
pub fn measure_throughput(iters: u64, samples_per_iter: u64, mut f: impl FnMut()) -> f64 {
    for _ in 0..iters {
        f();
    }
    let samples = (iters as f64) * (samples_per_iter as f64);
    let mut best = 0.0f64;
    for _ in 0..TIMED_BATCHES {
        let start = Instant::now();
        for _ in 0..iters {
            f();
        }
        let elapsed = start.elapsed().as_secs_f64().max(1e-9);
        best = best.max(samples / elapsed / 1e6);
    }
    best
}
