use super::*;

pub(super) struct Control {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<u64>>,
    load: Vec<JoinHandle<()>>,
}

impl Control {
    pub(super) fn start(engine: Arc<Engine>, sets: Vec<(u32, Vec<u32>)>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let load = (0..number("SDRMM_CAPTURE_CPU_THREADS", 0))
            .map(|_| {
                let stopped = stop.clone();
                std::thread::spawn(move || {
                    let mut value = 0.5f64;
                    while !stopped.load(Ordering::Acquire) {
                        for _ in 0..4096 {
                            value = std::hint::black_box(value).mul_add(0.999_999, 0.000_001);
                        }
                        std::hint::black_box(value);
                    }
                })
            })
            .collect();
        let retune = enabled("SDRMM_CAPTURE_RETUNE");
        let retune_device = enabled("SDRMM_CAPTURE_DEVICE_RETUNE");
        let worker = std::thread::spawn(move || {
            let started = Instant::now();
            let mut retunes = 0;
            while !stopped.load(Ordering::Acquire) {
                if (retune || retune_device) && started.elapsed().as_secs() / 5 > retunes {
                    retunes += 1;
                    for (ds, ids) in &sets {
                        if retune_device {
                            let started = Instant::now();
                            engine
                                .patch_device(
                                    *ds,
                                    DeviceSettings {
                                        center_hz: Some(
                                            100_000_000.0
                                                + if retunes % 2 == 0 { 0.0 } else { 1000.0 },
                                        ),
                                        ..Default::default()
                                    },
                                )
                                .expect("retune radio through USB");
                            if started.elapsed() > Duration::from_millis(100) {
                                eprintln!(
                                    "slow device retune ds={ds} elapsed={:?}",
                                    started.elapsed()
                                );
                            }
                        }
                        if !retune {
                            continue;
                        }
                        for (index, id) in ids.iter().enumerate() {
                            if stopped.load(Ordering::Acquire) {
                                return retunes;
                            }
                            let mut settings = channel_settings(index);
                            settings.frequency_hz += if retunes % 2 == 0 { 0.0 } else { 1000.0 };
                            let started = Instant::now();
                            engine
                                .patch_channel(*ds, *id, settings)
                                .expect("retune channel");
                            if started.elapsed() > Duration::from_millis(100) {
                                eprintln!(
                                    "slow retune ds={ds} ch={id} elapsed={:?}",
                                    started.elapsed()
                                );
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            retunes
        });
        Self {
            stop,
            worker: Some(worker),
            load,
        }
    }

    pub(super) fn finish(&mut self) -> u64 {
        self.stop.store(true, Ordering::Release);
        for worker in self.load.drain(..) {
            worker.join().expect("CPU stress thread");
        }
        self.worker
            .take()
            .map(|worker| worker.join().expect("control thread"))
            .unwrap_or(0)
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.finish();
    }
}
