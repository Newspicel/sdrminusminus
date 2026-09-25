use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use anyhow::anyhow;
use sdrmm_wire::AudioRoute;

use super::{
    CHANNELS, Health, SAMPLE_RATE,
    mixer::VoiceStats,
    monitor::PcmListener,
    output::Output,
    sink::{Opus, Sink},
};

struct Playing {
    voice: u64,
    gain: f32,
    sink: Sink<Opus>,
}

#[derive(Default)]
pub struct Engine {
    output: Option<Output>,
    playing: HashMap<AudioRoute, Playing>,
    streams: HashMap<u16, AudioRoute>,
    pending: VecDeque<AudioRoute>,
    clickers: usize,
    clicks: Option<f32>,
}

impl Engine {
    fn output(&mut self) -> anyhow::Result<&mut Output> {
        if self.output.is_none() {
            let output = Output::open()?;
            output
                .clicks()
                .set(self.clicks.is_some(), self.clicks.unwrap_or(0.0));
            self.output = Some(output);
        }
        self.output
            .as_mut()
            .ok_or_else(|| anyhow!("no audio output"))
    }

    pub fn open(&mut self, route: &AudioRoute, gain: f32) -> anyhow::Result<()> {
        if self.playing.contains_key(route) {
            return Ok(());
        }
        let stats = Arc::new(VoiceStats::with_gain(gain));
        let (voice, feed) = self.output()?.add(stats.clone())?;
        let sink = match Opus::new() {
            Ok(decoder) => Sink::new(decoder, feed, stats),
            Err(error) => {
                self.forget_voice(voice);
                return Err(error);
            }
        };
        self.playing
            .insert(route.clone(), Playing { voice, gain, sink });
        self.pending.push_back(route.clone());
        Ok(())
    }

    pub fn close(&mut self, route: &AudioRoute) {
        if let Some(playing) = self.playing.remove(route) {
            self.forget_voice(playing.voice);
        }
        self.pending.retain(|pending| pending != route);
        self.streams.retain(|_, bound| bound != route);
        self.release_if_idle();
    }

    fn forget_voice(&mut self, voice: u64) {
        if let Some(output) = self.output.as_mut() {
            output.remove(voice);
        }
    }

    fn release_if_idle(&mut self) {
        if self.playing.is_empty() && self.clickers == 0 {
            self.output = None;
        }
    }

    pub fn set_gain(&mut self, route: &AudioRoute, gain: f32) {
        if let Some(playing) = self.playing.get_mut(route) {
            playing.gain = gain;
            playing.sink.stats().set_gain(gain);
        }
    }

    pub fn started(&mut self, stream_id: u16, route: AudioRoute) -> bool {
        self.pending.retain(|pending| *pending != route);
        let Some(playing) = self.playing.get_mut(&route) else {
            return false;
        };
        playing.sink.restart();
        self.streams.insert(stream_id, route);
        true
    }

    pub fn stopped(&mut self, stream_id: u16) -> Option<AudioRoute> {
        self.streams.remove(&stream_id)
    }

    pub fn refused(&mut self) -> Option<AudioRoute> {
        self.pending.pop_front()
    }

    pub fn disconnected(&mut self) {
        self.streams.clear();
        self.pending.clear();
    }

    pub fn feed(
        &mut self,
        route: &AudioRoute,
        timestamp: u64,
        layout: u8,
        packet: &[u8],
        taps: &[PcmListener],
    ) -> anyhow::Result<()> {
        let Some(playing) = self.playing.get_mut(route) else {
            return Ok(());
        };
        playing.sink.push(timestamp, layout, packet, |block| {
            for tap in taps {
                tap(block, CHANNELS);
            }
        })
    }

    pub fn health(&self, route: &AudioRoute) -> Option<Health> {
        let playing = self.playing.get(route)?;
        let stats = playing.sink.stats();
        let ms = |frames: f64| (frames * 1_000.0 / f64::from(SAMPLE_RATE)) as f32;
        Some(Health {
            buffered_ms: ms(f64::from(stats.buffered_frames())),
            trimmed_ms: ms(stats.trimmed_frames() as f64),
            lost_ms: ms(playing.sink.lost_frames() as f64),
            underruns: stats.underruns(),
        })
    }

    pub fn latency_ms(&self, route: &AudioRoute) -> Option<f64> {
        let output = self.output.as_ref()?;
        let buffered = self.playing.get(route)?.sink.stats().buffered_frames();
        Some(f64::from(buffered) * 1_000.0 / f64::from(SAMPLE_RATE) + output.latency_ms())
    }

    pub fn poll(&mut self) -> anyhow::Result<()> {
        let Some(output) = self.output.as_mut() else {
            return Ok(());
        };
        output.collect();
        if output.broken() {
            self.reroute()?;
        }
        Ok(())
    }

    fn reroute(&mut self) -> anyhow::Result<()> {
        self.output = None;
        self.output()?;
        let Some(output) = self.output.as_mut() else {
            return Err(anyhow!("no audio output"));
        };
        for playing in self.playing.values_mut() {
            let stats = Arc::new(VoiceStats::with_gain(playing.gain));
            let (voice, feed) = output.add(stats.clone())?;
            playing.voice = voice;
            playing.sink.rebind(feed, stats);
        }
        Ok(())
    }

    pub fn drop_all(&mut self) -> Vec<AudioRoute> {
        let routes = self.playing.drain().map(|(route, _)| route).collect();
        self.streams.clear();
        self.pending.clear();
        self.release_if_idle();
        routes
    }

    pub fn hold_clicks(&mut self) -> anyhow::Result<()> {
        self.output()?;
        self.clickers += 1;
        Ok(())
    }

    pub fn set_clicks(&mut self, strength: Option<f32>) {
        self.clicks = strength;
        if let Some(output) = self.output.as_ref() {
            output
                .clicks()
                .set(strength.is_some(), strength.unwrap_or(0.0));
        }
    }

    pub fn release_clicks(&mut self) {
        self.clickers = self.clickers.saturating_sub(1);
        if self.clickers == 0 {
            self.set_clicks(None);
        }
        self.release_if_idle();
    }
}
