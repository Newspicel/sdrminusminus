use std::{thread, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use sdrmm_channels::{ChannelCtx, ChannelOutputs, ChannelRx};
use sdrmm_dsp::Ddc;
use sdrmm_wire::{ChannelParams, ChannelSettings, Squelch};

use super::{convert::Fixture, signals::Signal, usage};

const SETTLE_POLL: Duration = Duration::from_millis(250);
const SETTLE_POLLS: usize = 240;
const IDLE_CPU_SECONDS: f64 = 0.002;

pub struct Decoded {
    pub keys: Vec<String>,
    pub cpu_seconds: f64,
}

pub fn decode(fixture: &Fixture, signal: &Signal, loops: usize) -> Result<Decoded> {
    let params: ChannelParams =
        serde_json::from_value(serde_json::json!({ "type": signal.channel, "settings": {} }))
            .with_context(|| format!("default settings for {}", signal.channel))?;
    let input_rate = sdrmm_channels::descriptors()
        .into_iter()
        .find(|d| d.type_id == signal.channel)
        .with_context(|| format!("no channel called {}", signal.channel))?
        .input_rate_hz;
    let settings = ChannelSettings {
        frequency_hz: signal.offset_hz,
        squelch: Squelch::Off,
        params,
        blanker: Default::default(),
    };
    let mut ddc =
        Ddc::new(fixture.rate, input_rate, signal.offset_hz).map_err(|e| anyhow!("{e}"))?;
    let mut filter = sdrmm_channels::channel_filter(&settings.params)?;
    let mut channel = sdrmm_channels::create(ChannelCtx { input_rate }, &settings)?;
    let mut tuned = Vec::new();
    let mut filtered = Vec::new();
    let mut out = ChannelOutputs::default();
    let mut keys = Vec::new();
    let start = usage::own()?;
    fixture.feed(loops, signal.tail_seconds, |block| {
        ddc.process(block, &mut tuned);
        filter.process(&tuned, &mut filtered);
        out.reset();
        channel.process(&filtered, &mut out);
        keys.extend(out.events.iter().filter_map(signal.key));
        Ok(())
    })?;
    settle(channel.as_mut(), &mut out, &mut keys, signal)?;
    Ok(Decoded {
        keys,
        cpu_seconds: usage::own()? - start,
    })
}

fn settle(
    channel: &mut dyn ChannelRx,
    out: &mut ChannelOutputs,
    keys: &mut Vec<String>,
    signal: &Signal,
) -> Result<()> {
    for _ in 0..SETTLE_POLLS {
        let before = usage::own()?;
        thread::sleep(SETTLE_POLL);
        out.reset();
        channel.process(&[], out);
        keys.extend(out.events.iter().filter_map(signal.key));
        if usage::own()? - before < IDLE_CPU_SECONDS {
            return Ok(());
        }
    }
    bail!("{} kept working long after its input ended", signal.name)
}
