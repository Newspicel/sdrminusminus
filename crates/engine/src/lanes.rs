use sdrmm_device::DeviceError;

use crate::{
    CaptureRuntime, Engine, EngineError, dc_block, lock_runtime,
    scanner::sweep::{fault_handler, rebuild_channels, swap_runtime},
};

impl Engine {
    pub(crate) fn restart_lanes(&self, ds: u32) -> Result<(), EngineError> {
        let (runtime, settings, blocking) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            (
                state.runtime.clone(),
                state.settings.clone(),
                dc_block(&state.capabilities, &state.settings),
            )
        };
        let (device, taps) = {
            let mut current = lock_runtime(&runtime);
            let taps = current.taps();
            (current.release_device(), taps)
        };
        let device = device
            .ok_or_else(|| EngineError::Device(DeviceError::Io("the radio is down".to_string())))?;
        let receiving = CaptureRuntime::start_with_taps(
            device,
            &settings,
            blocking,
            taps,
            fault_handler(self, ds),
        )?;
        swap_runtime(self, ds, receiving)?;
        rebuild_channels(self, ds);
        Ok(())
    }
}
