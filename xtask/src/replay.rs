use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs};
use sdrmm_dsp::Ddc;
use sdrmm_wire::{AudioProcessing, ChannelParams, ChannelSettings};

use crate::excerpt::Source;

const BLOCK: usize = 65_536;

#[derive(Args)]
pub struct Replay {
    pub input: PathBuf,
    #[arg(long)]
    pub params: String,
    #[arg(long, default_value_t = 0.0)]
    pub offset: f64,
    #[arg(long)]
    pub input_rate: Option<f64>,
    #[arg(long)]
    pub quiet_tail: Option<f64>,
}

pub fn run(args: &Replay) -> Result<()> {
    let params: ChannelParams =
        serde_json::from_str(&args.params).context("read the channel parameters as JSON")?;
    let mut source = Source::open(&args.input, args.input_rate, None)?;
    let device_rate = source.rate;

    let type_id = params.type_id().to_owned();
    let descriptor = sdrmm_channels::descriptors()
        .into_iter()
        .find(|d| d.type_id == type_id)
        .with_context(|| format!("no channel called {type_id}"))?;
    let input_rate = match descriptor.native_rate_range() {
        Some(_) => device_rate,
        None => descriptor.input_rate_hz,
    };
    let settings = ChannelSettings {
        offset_hz: args.offset,
        squelch_db: None,
        squelch_auto_db: None,
        params,
        audio: AudioProcessing::default(),
    };
    let mut ddc =
        Ddc::new(device_rate, input_rate, args.offset).map_err(|err| anyhow::anyhow!("{err}"))?;
    let mut filter = sdrmm_channels::channel_filter(&settings.params)?;
    let mut channel = sdrmm_channels::create(ChannelCtx { input_rate }, &settings)?;

    let mut block = vec![Complex::default(); BLOCK];
    let mut tuned = Vec::new();
    let mut filtered = Vec::new();
    let mut out = ChannelOutputs::default();
    let mut read = 0u64;
    let mut events = 0usize;
    let mut audio = 0usize;
    let tail = (args.quiet_tail.unwrap_or(0.0) * device_rate).round() as u64;
    let mut tail_left = tail;
    loop {
        let got = source.read(&mut block)?;
        let got = if got > 0 {
            read += got as u64;
            got
        } else if tail_left > 0 {
            let n = BLOCK.min(tail_left as usize);
            block[..n].fill(Complex::default());
            tail_left -= n as u64;
            n
        } else {
            break;
        };
        ddc.process(&block[..got], &mut tuned);
        filter.process(&tuned, &mut filtered);
        out.reset();
        channel.process(&filtered, &mut out);
        audio += out.audio_pcm.len();
        for event in &out.events {
            events += 1;
            println!("{:9.4} s  {event:?}", read as f64 / device_rate);
        }
        for image in &out.images {
            println!(
                "{:9.4} s  image {}x{}",
                read as f64 / device_rate,
                image.picture.width,
                image.picture.height
            );
        }
    }
    println!(
        "{events} events, {audio} audio samples over {:.3} s of {type_id} at {device_rate} Hz",
        read as f64 / device_rate
    );
    Ok(())
}
