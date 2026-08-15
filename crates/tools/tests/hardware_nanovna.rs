#![allow(clippy::expect_used)]

use sdrmm_tools::{NanoVnaTool, Tool};
use sdrmm_wire::{
    NanoVnaCalStep, NanoVnaCalibrateRequest, NanoVnaPortRequest, NanoVnaRequest, NanoVnaResult,
    NanoVnaSweepRequest, ToolRequest, ToolResponse,
};

fn run(request: NanoVnaRequest) -> NanoVnaResult {
    let response = NanoVnaTool::default()
        .run(ToolRequest::NanoVna(request))
        .expect("the instrument answers");
    let ToolResponse::NanoVna(result) = response else {
        panic!("a NanoVNA request is answered by the NanoVNA tool");
    };
    *result
}

fn port() -> String {
    std::env::var("NANOVNA_PORT").expect("set NANOVNA_PORT to the instrument's serial port")
}

#[test]
#[ignore = "needs a NanoVNA on a serial port"]
fn discovery_finds_the_attached_instrument_and_skips_everything_else() {
    let NanoVnaResult::Devices {
        devices,
        ignored_ports,
    } = run(NanoVnaRequest::ListDevices)
    else {
        panic!("discovery returns devices");
    };
    println!("devices: {devices:#?}\nignored: {ignored_ports:?}");
    assert!(!devices.is_empty(), "no NanoVNA found");
}

#[test]
#[ignore = "needs a NanoVNA on a serial port"]
fn the_instrument_describes_itself() {
    let NanoVnaResult::Device(report) = run(NanoVnaRequest::Describe(NanoVnaPortRequest {
        port: port(),
    })) else {
        panic!("describe returns a report");
    };
    println!("{report:#?}");
    assert!(!report.firmware.is_empty());
    assert!(!report.commands.is_empty());
}

#[test]
#[ignore = "needs a NanoVNA on a serial port"]
fn a_sweep_comes_back_with_the_points_it_was_asked_for() {
    let NanoVnaResult::Sweep(sweep) = run(NanoVnaRequest::Sweep(NanoVnaSweepRequest {
        port: port(),
        start_hz: 1_000_000,
        stop_hz: 30_000_000,
        points: 201,
        averages: 2,
    })) else {
        panic!("a sweep returns points");
    };
    assert_eq!(sweep.points.len(), 201);
    assert_eq!(sweep.points[0].frequency_hz, 1_000_000);
    assert_eq!(sweep.points[200].frequency_hz, 30_000_000);
    println!(
        "{} points in {} ms, firmware {}, cal {}",
        sweep.points.len(),
        sweep.elapsed_ms,
        sweep.device.firmware,
        sweep.device.calibration.raw
    );
    println!("first: {:?}", sweep.points[0]);
}

#[test]
#[ignore = "needs a NanoVNA on a serial port"]
fn the_calibration_state_reads_back() {
    let NanoVnaResult::Calibration(state) =
        run(NanoVnaRequest::Calibrate(NanoVnaCalibrateRequest {
            port: port(),
            range: None,
            step: NanoVnaCalStep::Status,
        }))
    else {
        panic!("a calibration step returns the state");
    };
    println!("{state:#?}");
}
