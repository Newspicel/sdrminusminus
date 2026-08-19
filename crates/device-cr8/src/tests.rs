use std::{
    ffi::{c_int, c_void},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use sdrmm_device::{DeviceError, RxSink};
use sdrmm_wire::{DeviceSettings, ExtraValue, GainValue, StreamSettings};

use super::*;
use crate::settings::{Step, plan};

#[derive(Default)]
struct Recorder {
    serials: Vec<String>,
    calls: Mutex<Vec<String>>,
    path: PathBuf,
}

impl Recorder {
    fn note(&self, call: String) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Cr8Api for Recorder {
    fn library_path(&self) -> &Path {
        &self.path
    }

    fn serials(&self) -> Result<Vec<String>, DeviceError> {
        Ok(self.serials.clone())
    }

    fn open(&self, serial: &str) -> Result<DevHandle, DeviceError> {
        if self.serials.iter().any(|known| known == serial) {
            self.note(format!("open {serial}"));
            Ok(DevHandle(std::ptr::dangling_mut()))
        } else {
            Err(DeviceError::NotFound(format!("cr8:{serial}")))
        }
    }

    fn close(&self, _dev: DevHandle) {
        self.note("close".to_owned());
    }

    fn versions(&self, _dev: DevHandle) -> ffi::DevInfo {
        ffi::DevInfo::default()
    }

    fn start(
        &self,
        _dev: DevHandle,
        buffer: usize,
        _callback: ffi::Callback,
        _ctx: *mut c_void,
    ) -> Result<(), DeviceError> {
        self.note(format!("start {buffer}"));
        Ok(())
    }

    fn stop(&self, _dev: DevHandle) -> Result<(), DeviceError> {
        self.note("stop".to_owned());
        Ok(())
    }

    fn enable(&self, _dev: DevHandle, channels: c_int) -> Result<(), DeviceError> {
        self.note(format!("enable {channels:#x}"));
        Ok(())
    }

    fn disable(&self, _dev: DevHandle, channels: c_int) -> Result<(), DeviceError> {
        self.note(format!("disable {channels:#x}"));
        Ok(())
    }

    fn set_freq(
        &self,
        _dev: DevHandle,
        channels: c_int,
        freq_hz: f64,
        coherent: bool,
    ) -> Result<(), DeviceError> {
        self.note(format!("freq {channels:#x} {freq_hz} coherent={coherent}"));
        Ok(())
    }

    fn set_lna_gain(&self, _dev: DevHandle, channels: c_int, gain: i32) -> Result<(), DeviceError> {
        self.note(format!("lna {channels:#x} {gain}"));
        Ok(())
    }

    fn set_mixer_gain(
        &self,
        _dev: DevHandle,
        channels: c_int,
        gain: i32,
    ) -> Result<(), DeviceError> {
        self.note(format!("mixer {channels:#x} {gain}"));
        Ok(())
    }

    fn set_vga_gain(&self, _dev: DevHandle, channels: c_int, gain: i32) -> Result<(), DeviceError> {
        self.note(format!("vga {channels:#x} {gain}"));
        Ok(())
    }

    fn set_clock(&self, _dev: DevHandle, clock: c_int) -> Result<(), DeviceError> {
        self.note(format!("clock {clock}"));
        Ok(())
    }
}

fn recorder(serials: &[&str]) -> Arc<Recorder> {
    Arc::new(Recorder {
        serials: serials.iter().map(|serial| (*serial).to_owned()).collect(),
        ..Recorder::default()
    })
}

#[test]
fn a_machine_without_the_library_finds_no_radios() {
    assert!(Cr8Driver::new().probe().is_empty());
}

#[test]
fn every_serial_the_library_lists_is_offered_as_one_eight_lane_radio() {
    let api = recorder(&["DL0001", "DL0002"]);
    let driver = Cr8Driver::with_api(api);
    let found = driver.probe();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id(), "cr8:DL0001");
    let profile = found[0].profile.as_ref().expect("a profile");
    assert_eq!(profile.rx_streams, 8);
    assert_eq!(profile.sample_rates, vec![ffi::SAMPLE_RATE_HZ]);
    assert!(!profile.per_stream.tuning, "eight channels, one frequency");
}

#[test]
fn the_capabilities_say_the_lanes_share_a_synthesizer() {
    let capabilities = capabilities();
    assert_eq!(capabilities.rx_streams, 8);
    assert_eq!(capabilities.coherence, sdrmm_wire::Coherence::PhaseCoherent);
    assert_eq!(capabilities.gains.len(), 3);
    assert!(capabilities.per_stream.gain, "gain is per channel");
}

#[test]
fn a_radio_that_is_not_plugged_in_is_refused_by_name() {
    let driver = Cr8Driver::with_api(recorder(&["DL0001"]));
    let missing = DeviceInfo {
        driver: DRIVER_ID.to_owned(),
        key: "DL9999".to_owned(),
        label: String::new(),
        serial: None,
        profile: None,
    };
    let Err(DeviceError::NotFound(named)) = driver.open(&missing) else {
        panic!("a serial the library does not list must be refused");
    };
    assert!(named.contains("DL9999"), "{named}");
}

#[test]
fn tuning_moves_every_channel_together() {
    let capabilities = capabilities();
    let steps = plan(
        &DeviceSettings {
            center_hz: Some(433.92e6),
            ..DeviceSettings::default()
        },
        &DeviceSettings::default(),
        &capabilities,
    )
    .expect("a plain retune");
    assert_eq!(
        steps,
        vec![Step::Tune {
            channels: ffi::CHAN_ALL,
            freq_hz: 433.92e6
        }]
    );
}

#[test]
fn a_gain_meant_for_one_channel_reaches_only_that_channel() {
    let capabilities = capabilities();
    let steps = plan(
        &DeviceSettings {
            gains: vec![GainValue {
                stage: "LNA".to_owned(),
                value_db: 9.0,
            }],
            streams: vec![StreamSettings {
                stream: 2,
                center_hz: None,
                gains: vec![GainValue {
                    stage: "VGA".to_owned(),
                    value_db: 40.0,
                }],
                antenna: None,
            }],
            ..DeviceSettings::default()
        },
        &DeviceSettings::default(),
        &capabilities,
    )
    .expect("gains for everyone and for one");
    assert_eq!(
        steps,
        vec![
            Step::Lna {
                channels: ffi::CHAN_ALL,
                gain: 9
            },
            Step::Vga {
                channels: 0b100,
                gain: 15
            },
        ],
        "the stream gain is clamped to what the stage can reach"
    );
}

#[test]
fn a_stage_the_radio_does_not_have_is_refused_by_name() {
    let capabilities = capabilities();
    let Err(DeviceError::Unsupported(message)) = plan(
        &DeviceSettings {
            gains: vec![GainValue {
                stage: "IF".to_owned(),
                value_db: 3.0,
            }],
            ..DeviceSettings::default()
        },
        &DeviceSettings::default(),
        &capabilities,
    ) else {
        panic!("an unknown gain stage must be refused");
    };
    assert!(message.contains("IF"), "{message}");
}

#[test]
fn the_one_sample_rate_the_radio_has_is_the_only_one_accepted() {
    let capabilities = capabilities();
    assert!(
        plan(
            &DeviceSettings {
                sample_rate: Some(ffi::SAMPLE_RATE_HZ),
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        )
        .is_ok()
    );
    let Err(DeviceError::Unsupported(message)) = plan(
        &DeviceSettings {
            sample_rate: Some(2.4e6),
            ..DeviceSettings::default()
        },
        &DeviceSettings::default(),
        &capabilities,
    ) else {
        panic!("the CR-8 has one rate and settings that ask for another must say so");
    };
    assert!(message.contains("12.5"), "{message}");
}

#[test]
fn the_clock_source_is_chosen_before_anything_is_tuned_to_it() {
    let capabilities = capabilities();
    let steps = plan(
        &DeviceSettings {
            center_hz: Some(100e6),
            extra: vec![ExtraValue {
                name: CLOCK_SETTING.to_owned(),
                value: serde_json::Value::String(CLOCK_EXTERNAL.to_owned()),
            }],
            ..DeviceSettings::default()
        },
        &DeviceSettings::default(),
        &capabilities,
    )
    .expect("an external reference and a frequency");
    assert_eq!(steps[0], Step::Clock(ffi::CLOCK_EXTERNAL));
    assert!(matches!(steps[1], Step::Tune { .. }));
}

#[test]
fn a_stream_the_radio_does_not_have_is_refused_by_number() {
    let capabilities = capabilities();
    let Err(DeviceError::Unsupported(message)) = plan(
        &DeviceSettings {
            streams: vec![StreamSettings {
                stream: 9,
                center_hz: None,
                gains: vec![GainValue {
                    stage: "LNA".to_owned(),
                    value_db: 1.0,
                }],
                antenna: None,
            }],
            ..DeviceSettings::default()
        },
        &DeviceSettings::default(),
        &capabilities,
    ) else {
        panic!("stream nine on an eight-channel radio must be refused");
    };
    assert!(message.contains('9'), "{message}");
}

#[test]
fn starting_enables_every_channel_and_stopping_gives_them_back() {
    let api = recorder(&["DL0001"]);
    let mut device = Cr8Device::new(api.clone(), DevHandle(std::ptr::dangling_mut()));
    let sinks = (0..8).map(|_| RxSink::new(|_, _| {})).collect();
    device.rx_start(sinks).expect("starts");
    device.rx_stop();
    assert_eq!(
        api.calls(),
        vec![
            "enable 0xff".to_owned(),
            format!("start {BUFFER_SAMPLES}"),
            "stop".to_owned(),
            "disable 0xff".to_owned(),
        ]
    );
}

#[test]
fn the_wrong_number_of_sinks_is_refused_by_count() {
    let mut device = Cr8Device::new(recorder(&["DL0001"]), DevHandle(std::ptr::dangling_mut()));
    let Err(DeviceError::Unsupported(message)) = device.rx_start(vec![RxSink::new(|_, _| {})])
    else {
        panic!("one sink for an eight-lane radio must be refused");
    };
    assert!(message.contains('8'), "{message}");
}

#[test]
fn a_buffer_the_library_could_not_deliver_shows_up_as_a_gap_on_every_lane() {
    let seen: Arc<Mutex<Vec<(usize, u64, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let sinks: Vec<RxSink> = (0..8)
        .map(|lane| {
            let seen = seen.clone();
            RxSink::new(move |block: &[num_complex::Complex<f32>], index| {
                seen.lock().unwrap_or_else(PoisonError::into_inner).push((
                    lane,
                    index,
                    block.len(),
                ));
            })
        })
        .collect();
    let lanes = Arc::new(Mutex::new(Lanes { sinks }));
    let ctx = Arc::as_ptr(&lanes).cast::<c_void>().cast_mut();

    let mut buffers: Vec<Vec<ffi::Complex>> =
        (0..8).map(|_| vec![ffi::Complex::default(); 4]).collect();
    let mut pointers: Vec<*mut ffi::Complex> = buffers
        .iter_mut()
        .map(|buffer| buffer.as_mut_ptr())
        .collect();
    unsafe { deliver(pointers.as_mut_ptr(), 4, 0, ctx) };
    unsafe { deliver(pointers.as_mut_ptr(), 4, 100, ctx) };

    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let lane_three: Vec<(u64, usize)> = seen
        .iter()
        .filter(|(lane, _, _)| *lane == 3)
        .map(|(_, index, count)| (*index, *count))
        .collect();
    assert_eq!(
        lane_three,
        vec![(0, 4), (104, 4)],
        "the hundred samples the radio lost are stepped over, not silently closed up"
    );
    assert_eq!(seen.len(), 16, "every lane hears about every buffer");
}

#[test]
fn a_sample_is_laid_out_the_way_the_engine_reads_it() {
    assert_eq!(
        std::mem::size_of::<ffi::Complex>(),
        std::mem::size_of::<num_complex::Complex<f32>>()
    );
    assert_eq!(
        std::mem::align_of::<ffi::Complex>(),
        std::mem::align_of::<num_complex::Complex<f32>>()
    );
}

#[test]
fn channels_are_named_by_the_bit_the_library_expects() {
    assert_eq!(channel_mask(0), 0b1);
    assert_eq!(channel_mask(7), 0b1000_0000);
    assert_eq!(channel_mask(8), 0, "there is no ninth channel");
}
