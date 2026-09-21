use std::{
    env,
    error::Error,
    fs::File,
    io::{BufReader, Read},
};

use num_complex::Complex;
use sdrmm_channels::{
    ChannelCtx, ChannelOutputs, channel_filter, create,
    monitor::{MonitorOutput, SpectrumMonitor},
};
use sdrmm_dsp::Ddc;
use sdrmm_wire::{ChannelSettings, SpectrumMonitorNode, TransmissionState};

fn emit(output: Vec<MonitorOutput>) -> Result<(), Box<dyn Error>> {
    for out in output {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "transmission": out.transmission,
                "frequency_hz": out.frequency_hz,
                "audio_samples": out.audio.len(),
                "event": out.event,
            }))?
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    if !(4..=5).contains(&args.len()) {
        return Err("usage: monitor_iq <cf32_le file> <sample rate> <center Hz> [DMR Hz]".into());
    }
    let rate = args[2].parse::<f64>()?;
    let center = args[3].parse::<f64>()?;
    let mut reader = BufReader::new(File::open(&args[1])?);
    let mut monitor = SpectrumMonitor::new(rate, center, SpectrumMonitorNode::default())?;
    let settings = ChannelSettings::default_for("dmr").ok_or("missing DMR decoder")?;
    let mut decoder = create(
        ChannelCtx {
            input_rate: 48_000.0,
        },
        &settings,
    )?;
    let mut filter = channel_filter(&settings.params)?;
    let frequency = args.get(4).map(|arg| arg.parse::<f64>()).transpose()?;
    let mut ddc = Ddc::new(rate, 48_000.0, frequency.unwrap_or(center) - center)?;
    let mut tuned = Vec::new();
    let mut filtered = Vec::new();
    let mut out = ChannelOutputs::default();
    let mut bytes = vec![0u8; 8192 * 8];
    let mut samples = Vec::with_capacity(8192);
    let mut position = 0;
    loop {
        let mut count = 0;
        while count < bytes.len() {
            let read = reader.read(&mut bytes[count..])?;
            if read == 0 {
                break;
            }
            count += read;
        }
        if count == 0 {
            break;
        }
        if count % 8 != 0 {
            return Err("incomplete IQ sample".into());
        }
        samples.clear();
        for sample in bytes[..count].as_chunks::<8>().0 {
            samples.push(Complex::new(
                f32::from_le_bytes(sample[..4].try_into()?),
                f32::from_le_bytes(sample[4..].try_into()?),
            ));
        }
        if frequency.is_some() {
            ddc.process(&samples, &mut tuned);
            filter.process(&tuned, &mut filtered);
            out.reset();
            decoder.process(&filtered, &mut out);
            for event in &out.events {
                println!("{}", serde_json::to_string(event)?);
            }
        } else {
            emit(monitor.process(&samples, position))?;
        }
        position += samples.len() as u64;
    }
    emit(monitor.finish(TransmissionState::Completed, None))?;
    eprintln!("{} seconds", position as f64 / rate);
    Ok(())
}
