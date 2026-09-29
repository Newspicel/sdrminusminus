use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use sdrmm_device::{DeviceDriver, GapScope, LaneEvent, SinkItem, SinkRoom, Uncertainty};
use sdrmm_recorder::{CollectionArray, CollectionWriter, LaneMeta};
use sdrmm_wire::{ArrayGeometry, Coherence, DcArtifact, DeviceInfo, LaneKey};
use tempfile::TempDir;

use super::*;
use crate::RecordingDriver;

const RATE: f64 = 250_000.0;
const LANES: usize = 3;

#[derive(Debug, PartialEq)]
enum Seen {
    Samples {
        index: u64,
        samples: Vec<Complex<f32>>,
    },
    Event(LaneEvent),
}

fn value(lane: usize, i: usize) -> Complex<f32> {
    Complex::new(lane as f32 + 1.0, i as f32)
}

fn record(stem: &Path, build: impl FnOnce(&mut CollectionWriter)) {
    let lanes: Vec<LaneMeta> = (0..LANES)
        .map(|stream| LaneMeta {
            lane: LaneKey {
                device: "virtual:kraken5".to_owned(),
                stream: stream as u32,
            },
            center_hz: 100e6 + stream as f64 * 1e3,
        })
        .collect();
    let array = CollectionArray {
        node: "array".to_owned(),
        tier: Coherence::TimeSync,
        geometry: ArrayGeometry::default(),
        noise_source: NoiseSource::Isolated,
        retune_keeps_phase: true,
        dc_artifact: DcArtifact::Managed,
    };
    let mut writer = CollectionWriter::create(stem, &lanes, RATE, array).unwrap();
    build(&mut writer);
    writer.finalize().unwrap();
}

fn write(writer: &mut CollectionWriter, start: usize, n: usize) {
    let blocks: Vec<Vec<Complex<f32>>> = (0..LANES)
        .map(|lane| (start..start + n).map(|i| value(lane, i)).collect())
        .collect();
    let views: Vec<&[Complex<f32>]> = blocks.iter().map(Vec::as_slice).collect();
    writer.write(&views).unwrap();
}

fn open(dir: &Path, key: &str) -> Box<dyn SdrDevice> {
    let driver = RecordingDriver::new(Some(dir.to_path_buf()));
    let info = driver
        .probe()
        .into_iter()
        .find(|info| info.key == key)
        .unwrap();
    driver.open(&info).unwrap()
}

fn play(device: &mut dyn SdrDevice, samples: usize) -> Vec<Vec<Seen>> {
    let mut receivers = Vec::new();
    let sinks = (0..LANES)
        .map(|_| {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            RxSink::with_items(
                move |item| {
                    let seen = match item {
                        SinkItem::Samples { samples, index } => Seen::Samples {
                            index,
                            samples: samples.to_vec(),
                        },
                        SinkItem::Event(event) => Seen::Event(event),
                    };
                    let _ = tx.send(seen);
                },
                |err| panic!("playback failed: {err}"),
            )
        })
        .collect();
    device.rx_start(sinks).unwrap();
    let lanes = receivers
        .iter()
        .map(|rx| {
            let mut seen = Vec::new();
            let mut got = 0;
            while got < samples {
                let item = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if let Seen::Samples { samples, .. } = &item {
                    got += samples.len();
                }
                seen.push(item);
            }
            seen
        })
        .collect();
    device.rx_stop();
    lanes
}

fn samples_of(seen: &[Seen]) -> Vec<(u64, Complex<f32>)> {
    seen.iter()
        .flat_map(|item| match item {
            Seen::Samples { index, samples } => samples
                .iter()
                .enumerate()
                .map(|(i, s)| (index + i as u64, *s))
                .collect(),
            Seen::Event(_) => Vec::new(),
        })
        .collect()
}

fn events_of(seen: &[Seen]) -> Vec<LaneEvent> {
    seen.iter()
        .filter_map(|item| match item {
            Seen::Event(event) => Some(*event),
            Seen::Samples { .. } => None,
        })
        .collect()
}

#[test]
fn a_collection_plays_as_a_multi_lane_device() {
    let dir = TempDir::new().unwrap();
    record(&dir.path().join("bank"), |writer| {
        write(writer, 0, 100);
        writer.gap(50).unwrap();
        write(writer, 100, 60);
    });
    let driver = RecordingDriver::new(Some(dir.path().to_path_buf()));
    let ids: Vec<String> = driver.probe().iter().map(DeviceInfo::id).collect();
    assert_eq!(
        ids,
        vec!["recording:bank"],
        "lane files hide behind their collection"
    );
    assert!(driver.resolve("bank").is_some());

    let mut device = open(dir.path(), "bank");
    let caps = device.capabilities();
    assert_eq!(caps.rx_streams, 3);
    assert_eq!(caps.coherence, Coherence::TimeSync);
    assert_eq!(caps.noise_source, NoiseSource::Replayed);
    assert_eq!(caps.dc_artifact, DcArtifact::Managed);
    assert!(caps.retune_keeps_phase);
    assert!(caps.per_stream.tuning);
    assert_eq!(caps.sample_rates, vec![RATE]);
    assert_eq!(device.in_flight_samples(), 0);
    let settings = device.settings().clone();
    assert_eq!(settings.center_hz, Some(100e6));
    assert_eq!(settings.streams[2].center_hz, Some(100.002e6));
    assert!(
        device
            .apply(&DeviceSettings {
                center_hz: Some(101e6),
                ..DeviceSettings::default()
            })
            .is_err()
    );
    assert!(device.rx_start(vec![RxSink::new(|_, _| {})]).is_err());

    let lanes = play(device.as_mut(), 160);
    for (lane, seen) in lanes.iter().enumerate() {
        let samples = samples_of(seen);
        let expected: Vec<(u64, Complex<f32>)> = (0..160)
            .map(|i| {
                let index = if i < 100 { i } else { i + 50 };
                (index as u64, value(lane, i))
            })
            .collect();
        assert_eq!(samples, expected, "lane {lane}");
    }
}

#[test]
fn recorded_noise_windows_come_back_as_marks() {
    let dir = TempDir::new().unwrap();
    record(&dir.path().join("cal"), |writer| {
        write(writer, 0, 80);
        writer.noise(20, 40).unwrap();
        writer
            .retuned(&[101e6, 101e6, 101e6], "2026-09-28T12:00:00Z")
            .unwrap();
        write(writer, 80, 20);
    });
    let mut device = open(dir.path(), "cal");
    let lanes = play(device.as_mut(), 100);
    for seen in &lanes {
        assert_eq!(
            events_of(seen),
            vec![
                LaneEvent::Mark {
                    at: 20,
                    mark: LaneMark::NoiseSource {
                        on: true,
                        in_flight: 0
                    }
                },
                LaneEvent::Mark {
                    at: 60,
                    mark: LaneMark::NoiseSource {
                        on: false,
                        in_flight: 0
                    }
                },
                LaneEvent::Mark {
                    at: 80,
                    mark: LaneMark::Retuned { in_flight: 0 }
                },
            ]
        );
    }
}

#[test]
fn every_lane_is_told_when_the_collection_ends() {
    let dir = TempDir::new().unwrap();
    record(&dir.path().join("short"), |writer| {
        write(writer, 0, 60);
        writer.noise(10, 20).unwrap();
    });
    let mut device = open(dir.path(), "short");
    let mut receivers = Vec::new();
    let sinks = (0..LANES)
        .map(|_| {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            RxSink::with_items(
                move |item| {
                    if let SinkItem::Event(event) = item {
                        let _ = tx.send(event);
                    }
                },
                |err| panic!("playback failed: {err}"),
            )
        })
        .collect();
    device.rx_start(sinks).unwrap();
    for rx in &receivers {
        let events: Vec<LaneEvent> =
            std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(5)).ok())
                .take(3)
                .collect();
        assert_eq!(
            events.last(),
            Some(&LaneEvent::Mark {
                at: 60,
                mark: LaneMark::Ended
            })
        );
    }
    device.rx_stop();
}

#[test]
fn a_recorded_realignment_unsettles_only_the_lanes_it_moved() {
    let dir = TempDir::new().unwrap();
    record(&dir.path().join("moved"), |writer| {
        writer.offsets(&[0, 5, 9]).unwrap();
        write(writer, 0, 100);
        writer.offsets(&[0, 7, 9]).unwrap();
        write(writer, 100, 100);
    });
    let mut device = open(dir.path(), "moved");
    let lanes = play(device.as_mut(), 200);
    assert_eq!(
        events_of(&lanes[1]),
        vec![LaneEvent::Uncertain {
            at: 100,
            error: 2,
            scope: GapScope::Lane,
            cause: Uncertainty::Unaligned,
        }]
    );
    assert!(events_of(&lanes[0]).is_empty());
    assert!(events_of(&lanes[2]).is_empty());
}

#[test]
fn every_lane_waits_for_the_fullest_sink() {
    let dir = TempDir::new().unwrap();
    let piece = (RATE * BLOCK_SECS) as usize;
    record(&dir.path().join("held"), |writer| {
        write(writer, 0, piece * 3)
    });
    let mut device = open(dir.path(), "held");
    let rooms: Vec<Arc<SinkRoom>> = (0..LANES)
        .map(|lane| Arc::new(SinkRoom::new(if lane == 1 { 0 } else { piece * 3 })))
        .collect();
    let mut receivers = Vec::new();
    let sinks = rooms
        .iter()
        .map(|room| {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            let taken = room.clone();
            RxSink::new(move |samples: &[Complex<f32>], _| {
                taken.took(samples.len());
                let _ = tx.send(samples.len());
            })
            .with_room(room.clone())
        })
        .collect();
    device.rx_start(sinks).unwrap();
    for rx in &receivers {
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }
    rooms[1].freed(piece);
    for rx in &receivers {
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), piece);
    }
    assert!(
        receivers[0]
            .recv_timeout(Duration::from_millis(200))
            .is_err()
    );
    device.rx_stop();
}
