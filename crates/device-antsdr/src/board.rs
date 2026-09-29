use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

use sdrmm_device::DeviceError;

use crate::{
    ad9361::{Ad9361, Chains, Direction, GainMode, REFERENCE_HZ, Settings, Tracking},
    control::{Control, Target},
    radio::{self, Command},
    rate::{self, Plan},
    regs::{self, core},
    spi::{Spi, reference},
};

const INTERNAL_TIME: u32 = 2;
const START_MARGIN_S: f64 = 0.05;

#[derive(Debug)]
pub(crate) struct Timeline {
    generation: AtomicU32,
    decimation: AtomicU32,
    tick_bits: AtomicU64,
    rx_scale_bits: AtomicU32,
    tx_scale_bits: AtomicU32,
}

impl Timeline {
    pub(crate) fn new(plan: Plan) -> Self {
        let timeline = Self {
            generation: AtomicU32::new(0),
            decimation: AtomicU32::new(1),
            tick_bits: AtomicU64::new(0),
            rx_scale_bits: AtomicU32::new(0),
            tx_scale_bits: AtomicU32::new(0),
        };
        timeline.publish(plan);
        timeline
    }

    pub(crate) fn publish(&self, plan: Plan) {
        self.decimation.store(plan.decimation, Ordering::Release);
        self.tick_bits.store(plan.tick.to_bits(), Ordering::Release);
        self.rx_scale_bits.store(
            rate::decimator(plan.decimation).host_scale.to_bits(),
            Ordering::Release,
        );
        self.tx_scale_bits.store(
            rate::interpolator(plan.decimation).host_scale.to_bits(),
            Ordering::Release,
        );
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn ticks_per_sample(&self) -> u64 {
        u64::from(self.decimation.load(Ordering::Acquire).max(1))
    }

    pub(crate) fn tick(&self) -> f64 {
        f64::from_bits(self.tick_bits.load(Ordering::Acquire))
    }

    pub(crate) fn rx_scale(&self) -> f32 {
        f32::from_bits(self.rx_scale_bits.load(Ordering::Acquire))
    }

    pub(crate) fn tx_scale(&self) -> f32 {
        f32::from_bits(self.tx_scale_bits.load(Ordering::Acquire))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Misc {
    tx_band_high: bool,
    rx_band: u8,
    codec_reset: bool,
    mimo: bool,
}

impl Misc {
    fn word(self) -> u32 {
        let tx = if self.tx_band_high { 1 << 7 } else { 1 << 6 };
        let rx = match self.rx_band {
            0 => 1 << 3,
            1 => 1 << 4,
            _ => 1 << 5,
        };
        tx | rx | u32::from(self.codec_reset) << 2 | u32::from(self.mimo) << 1
    }

    fn follow(&mut self, rx_hz: f64, tx_hz: f64) {
        self.rx_band = if rx_hz < 2.2e9 {
            0
        } else if rx_hz < 4e9 {
            1
        } else {
            2
        };
        self.tx_band_high = tx_hz >= 2.5e9;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Start {
    pub(crate) rate: f64,
    pub(crate) lanes: usize,
    pub(crate) center_hz: f64,
    pub(crate) bandwidth: f64,
}

pub(crate) struct Board {
    control: Arc<Control>,
    chip: Ad9361<Spi>,
    misc: Misc,
    radios: usize,
    plan: Plan,
    rate: f64,
    lanes: usize,
    streaming: [bool; 2],
    transmitting: [bool; 2],
    timeline: Arc<Timeline>,
}

impl Board {
    pub(crate) fn open(control: Arc<Control>, start: Start) -> Result<Self, DeviceError> {
        check_fpga(&control)?;
        let status = control.peek32(Target::Local, regs::STATUS_READBACK)?;
        let radios = ((status >> 8) & 0xff) as usize;
        if !(1..=2).contains(&radios) {
            return Err(DeviceError::Io(format!(
                "the FPGA reports {radios} radio chains"
            )));
        }
        let lanes = start.lanes.clamp(1, radios);
        let plan = rate::plan(start.rate, lanes)?;
        let mut misc = Misc::default();
        misc.follow(start.center_hz, start.center_hz);
        let mut spi = Spi::new(control.clone());
        spi.reference_pll(&reference::words(false))?;
        for reset in [true, false] {
            misc.codec_reset = reset;
            control.poke(Target::Local, core::MISC, misc.word())?;
        }
        let mut chip = Ad9361::new(spi, REFERENCE_HZ);
        chip.initialize(Settings {
            rate: plan.tick,
            rx_hz: start.center_hz,
            tx_hz: start.center_hz,
            bandwidth: start.bandwidth,
        })?;
        for lane in 0..radios {
            radio::register_self_test(&control, lane)?;
            radio::prepare(&control, lane)?;
            radio::frame_tx(&control, lane)?;
        }
        chip.set_loopback(true)?;
        std::thread::sleep(std::time::Duration::from_millis(1));
        let tested = (0..radios).try_for_each(|lane| radio::interface_self_test(&control, lane));
        chip.set_loopback(false)?;
        tested?;
        radio::zero_time(&control, radios, INTERNAL_TIME)?;
        let mut board = Self {
            timeline: Arc::new(Timeline::new(plan)),
            control,
            chip,
            misc,
            radios,
            plan,
            rate: start.rate,
            lanes,
            streaming: [false; 2],
            transmitting: [false; 2],
        };
        board.load_stages()?;
        board.update_chains()?;
        Ok(board)
    }

    pub(crate) const fn radios(&self) -> usize {
        self.radios
    }

    pub(crate) fn timeline(&self) -> Arc<Timeline> {
        self.timeline.clone()
    }

    pub(crate) const fn rate(&self) -> f64 {
        self.rate
    }

    pub(crate) fn settle(&self) -> Result<(), DeviceError> {
        self.control.settle()
    }

    pub(crate) fn frequency(&self) -> f64 {
        self.chip.frequency(Direction::Rx)
    }

    pub(crate) fn set_rate(&mut self, rate: f64, lanes: usize) -> Result<f64, DeviceError> {
        let lanes = lanes.clamp(1, self.radios);
        let plan = rate::plan(rate, lanes)?;
        let running = self.running();
        for lane in &running {
            radio::stream(&self.control, *lane, Command::Stop)?;
        }
        if (plan.tick - self.plan.tick).abs() >= 1.0 {
            self.chip.set_rate(plan.tick)?;
            radio::zero_time(&self.control, self.radios, INTERNAL_TIME)?;
        }
        self.plan = plan;
        self.rate = rate;
        self.lanes = lanes;
        self.load_stages()?;
        self.timeline.publish(plan);
        start_lanes(&self.control, &running, &self.timeline)?;
        Ok(rate)
    }

    pub(crate) fn tune(&mut self, hz: f64) -> Result<f64, DeviceError> {
        let rx = self.chip.tune(Direction::Rx, hz)?;
        let tx = self.chip.tune(Direction::Tx, hz)?;
        self.misc.follow(rx, tx);
        self.control
            .poke(Target::Local, core::MISC, self.misc.word())?;
        Ok(rx)
    }

    pub(crate) fn set_rx_gain(&mut self, lane: usize, db: f64) -> Result<f64, DeviceError> {
        self.chip.set_rx_gain(lane, db)
    }

    pub(crate) fn set_tx_gain(&mut self, lane: usize, db: f64) -> Result<f64, DeviceError> {
        Ok(-self.chip.set_tx_attenuation(lane, -db)?)
    }

    pub(crate) fn set_gain_mode(&mut self, lane: usize, mode: GainMode) -> Result<(), DeviceError> {
        self.chip.set_gain_mode(lane, mode)
    }

    pub(crate) fn gain(&mut self, lane: usize) -> Result<f64, DeviceError> {
        self.chip.gain_index(lane)
    }

    pub(crate) fn set_bandwidth(&mut self, hz: f64) -> Result<f64, DeviceError> {
        self.chip.set_bandwidth(hz)
    }

    pub(crate) fn set_tracking(&mut self, tracking: Tracking) -> Result<(), DeviceError> {
        self.chip.set_tracking(tracking)
    }

    pub(crate) fn set_ppm(&mut self, ppm: f64) -> Result<(), DeviceError> {
        self.chip.set_reference(REFERENCE_HZ * (1.0 + ppm / 1e6))
    }

    pub(crate) fn set_streaming(&mut self, lanes: usize) -> Result<(), DeviceError> {
        self.streaming = [lanes > 0, lanes > 1];
        self.check_combination()?;
        self.update_chains()
    }

    pub(crate) fn set_transmitting(&mut self, lanes: usize) -> Result<(), DeviceError> {
        let before = self.transmitting;
        self.transmitting = [lanes > 0, lanes > 1];
        if let Err(e) = self.check_combination() {
            self.transmitting = before;
            return Err(e);
        }
        for lane in 0..lanes {
            radio::frame_tx(&self.control, lane)?;
        }
        self.update_chains()
    }

    fn check_combination(&self) -> Result<(), DeviceError> {
        let rx = self.streaming.iter().filter(|on| **on).count();
        let tx = self.transmitting.iter().filter(|on| **on).count();
        if rx + tx == 3 {
            return Err(DeviceError::Unsupported(format!(
                "the AD9361 cannot run {rx} receive and {tx} transmit lanes together"
            )));
        }
        if rx.max(tx) > self.lanes {
            return Err(DeviceError::Unsupported(format!(
                "the rate is set for {} lanes; set Lanes to 2 first",
                self.lanes
            )));
        }
        Ok(())
    }

    fn running(&self) -> Vec<usize> {
        (0..self.radios)
            .filter(|lane| self.streaming[*lane])
            .collect()
    }

    fn load_stages(&mut self) -> Result<(), DeviceError> {
        let decimator = rate::decimator(self.plan.decimation);
        let interpolator = rate::interpolator(self.plan.decimation);
        for lane in 0..self.radios {
            radio::set_decimator(&self.control, lane, decimator)?;
            radio::set_interpolator(&self.control, lane, interpolator)?;
        }
        self.control.settle()
    }

    fn update_chains(&mut self) -> Result<(), DeviceError> {
        let idle = !self
            .streaming
            .iter()
            .chain(&self.transmitting)
            .any(|on| *on);
        let chains = if idle {
            Chains {
                rx: [true, false],
                tx: [true, false],
            }
        } else {
            Chains {
                rx: self.streaming,
                tx: self.transmitting,
            }
        };
        self.chip.set_chains(chains)?;
        let rx = self.streaming.iter().filter(|on| **on).count();
        let tx = self.transmitting.iter().filter(|on| **on).count();
        self.misc.mimo = rx == 2 || tx == 2;
        self.control
            .poke(Target::Local, core::MISC, self.misc.word())?;
        for lane in 0..self.radios {
            radio::set_switches(
                &self.control,
                lane,
                self.streaming[lane],
                self.transmitting[lane],
            )?;
        }
        self.control.settle()
    }
}

fn check_fpga(control: &Control) -> Result<(), DeviceError> {
    let compat = control.peek64(Target::Local, regs::COMPAT)?;
    let signature = (compat >> 32) as u32;
    let major = (compat >> 16) as u16;
    if signature != regs::SIGNATURE {
        return Err(DeviceError::Io(format!(
            "no AntSDR UHD image answered (signature {signature:#010x})"
        )));
    }
    if major != regs::FPGA_COMPAT {
        return Err(DeviceError::Unsupported(format!(
            "FPGA image {major}.{} is not the supported {}",
            compat as u16,
            regs::FPGA_COMPAT
        )));
    }
    Ok(())
}

pub(crate) fn start_lanes(
    control: &Control,
    lanes: &[usize],
    timeline: &Timeline,
) -> Result<(), DeviceError> {
    match lanes {
        [] => Ok(()),
        [lane] => radio::stream(control, *lane, Command::StartNow),
        _ => {
            let now = radio::time_now(control)?;
            let at = now + (START_MARGIN_S * timeline.tick()) as u64;
            for lane in lanes {
                radio::stream(control, *lane, Command::StartAt(at))?;
            }
            Ok(())
        }
    }
}

pub(crate) fn stop_lanes(control: &Control, lanes: usize) -> Result<(), DeviceError> {
    (0..lanes).try_for_each(|lane| radio::stream(control, lane, Command::Stop))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_misc_word_carries_the_band_filters_and_the_reset() {
        let mut misc = Misc::default();
        misc.follow(100e6, 100e6);
        assert_eq!(misc.word(), 1 << 6 | 1 << 3);
        misc.follow(2.4e9, 2.4e9);
        assert_eq!(misc.word(), 1 << 6 | 1 << 4);
        misc.follow(5.8e9, 5.8e9);
        misc.mimo = true;
        misc.codec_reset = true;
        assert_eq!(misc.word(), 1 << 7 | 1 << 5 | 1 << 2 | 1 << 1);
    }

    #[test]
    fn a_new_rate_is_published_to_the_streams() {
        let timeline = Timeline::new(rate::plan(2.048e6, 1).expect("plan"));
        let before = timeline.generation();
        assert_eq!(timeline.ticks_per_sample(), 16);
        timeline.publish(rate::plan(10e6, 1).expect("plan"));
        assert_eq!(timeline.ticks_per_sample(), 4);
        assert!(timeline.generation() > before);
        assert!((timeline.tick() - 40e6).abs() < 1.0);
        assert!(timeline.rx_scale() > 0.0 && timeline.tx_scale() > 1.0);
    }
}
