use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

use sdrmm_channels::{AudioChain, ClickProfile};
use sdrmm_wire::{AudioProcessing, AudioRoute};
use tokio::sync::{
    broadcast::{self, error::RecvError},
    watch,
};

use crate::{
    EngineError,
    audio::{self, AudioPacket, PcmBlock, PcmPayload},
    audio_recording::AudioRecorderTap,
};

pub(crate) enum FxControl {
    Record(AudioRecorderTap),
    StopRecording,
}

pub(crate) struct FxSource {
    pub(crate) pcm: broadcast::Receiver<PcmBlock>,
    pub(crate) channels: u8,
    pub(crate) profile: ClickProfile,
    pub(crate) capacity: usize,
}

struct FxStream {
    pcm_tx: broadcast::WeakSender<PcmBlock>,
    audio_tx: broadcast::WeakSender<AudioPacket>,
    control: mpsc::Sender<FxControl>,
    worker: JoinHandle<()>,
}

impl FxStream {
    fn live(&self) -> bool {
        !self.worker.is_finished() && self.pcm_tx.strong_count() > 0
    }
}

#[derive(Default)]
pub(crate) struct AudioFxHub {
    settings: HashMap<String, watch::Sender<AudioProcessing>>,
    streams: HashMap<AudioRoute, FxStream>,
}

impl AudioFxHub {
    pub(crate) fn set(&mut self, node: &str, settings: AudioProcessing) {
        match self.settings.get(node) {
            Some(tx) => {
                tx.send_if_modified(|current| {
                    let changed = *current != settings;
                    if changed {
                        *current = settings;
                    }
                    changed
                });
            }
            None => {
                self.settings
                    .insert(node.to_owned(), watch::channel(settings).0);
            }
        }
    }

    pub(crate) fn retain(&mut self, nodes: &HashSet<String>) {
        self.settings.retain(|node, _| nodes.contains(node));
        self.prune();
    }

    fn prune(&mut self) {
        self.streams.retain(|_, stream| stream.live());
    }

    #[cfg(test)]
    pub(crate) fn has(&self, route: &AudioRoute) -> bool {
        self.streams.get(route).is_some_and(FxStream::live)
    }

    pub(crate) fn subscribe_pcm(
        &mut self,
        route: &AudioRoute,
        source: impl FnOnce() -> Result<FxSource, EngineError>,
    ) -> Result<broadcast::Receiver<PcmBlock>, EngineError> {
        self.stream(route, source)?
            .pcm_tx
            .upgrade()
            .map(|tx| tx.subscribe())
            .ok_or_else(stopped)
    }

    pub(crate) fn subscribe_audio(
        &mut self,
        route: &AudioRoute,
        source: impl FnOnce() -> Result<FxSource, EngineError>,
    ) -> Result<broadcast::Receiver<AudioPacket>, EngineError> {
        self.stream(route, source)?
            .audio_tx
            .upgrade()
            .map(|tx| tx.subscribe())
            .ok_or_else(stopped)
    }

    pub(crate) fn recorder(
        &mut self,
        route: &AudioRoute,
        source: impl FnOnce() -> Result<FxSource, EngineError>,
        tap: AudioRecorderTap,
    ) -> Result<mpsc::Sender<FxControl>, EngineError> {
        let control = self.stream(route, source)?.control.clone();
        control
            .send(FxControl::Record(tap))
            .map_err(|_| stopped())?;
        Ok(control)
    }

    fn stream(
        &mut self,
        route: &AudioRoute,
        source: impl FnOnce() -> Result<FxSource, EngineError>,
    ) -> Result<&FxStream, EngineError> {
        self.prune();
        if !self.streams.contains_key(route) {
            let stages = route
                .fx
                .iter()
                .map(|node| {
                    self.settings
                        .get(node)
                        .map(watch::Sender::subscribe)
                        .ok_or_else(|| {
                            EngineError::Audio(format!("audio FX `{node}` is not in the patch"))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let stream = spawn(source()?, stages)?;
            self.streams.insert(route.clone(), stream);
        }
        self.streams
            .get(route)
            .ok_or_else(|| EngineError::Audio("audio FX stream vanished".to_owned()))
    }
}

fn stopped() -> EngineError {
    EngineError::Audio("audio FX stream stopped".to_owned())
}

fn spawn(
    source: FxSource,
    settings: Vec<watch::Receiver<AudioProcessing>>,
) -> Result<FxStream, EngineError> {
    let (pcm_tx, pcm_rx) = broadcast::channel(source.capacity);
    let (audio_tx, _) = broadcast::channel(audio::AUDIO_CHANNEL_CAP);
    let (control, commands) = mpsc::channel();
    let encoder = audio::spawn_encoder(source.channels, pcm_rx, audio_tx.clone())?;
    let weak_pcm = pcm_tx.downgrade();
    let weak_audio = audio_tx.downgrade();
    let worker = {
        std::thread::Builder::new()
            .name("sdrmm-audiofx".to_owned())
            .spawn(move || {
                sdrmm_device::schedule::claim(sdrmm_device::Latency::Interactive);
                Worker {
                    stages: settings.into_iter().map(Stage::new).collect(),
                    profile: source.profile,
                    pcm_tx,
                    audio_tx,
                    commands,
                    recorder: None,
                    failed: false,
                }
                .run(source.pcm);
                drop(encoder);
            })
            .map_err(|e| EngineError::Audio(format!("spawn audio FX thread: {e}")))?
    };
    Ok(FxStream {
        pcm_tx: weak_pcm,
        audio_tx: weak_audio,
        control,
        worker,
    })
}

struct Stage {
    settings: watch::Receiver<AudioProcessing>,
    chain: Option<AudioChain>,
}

impl Stage {
    fn new(settings: watch::Receiver<AudioProcessing>) -> Self {
        Self {
            settings,
            chain: None,
        }
    }

    fn prepare(
        &mut self,
        channels: u8,
        profile: ClickProfile,
    ) -> Result<bool, sdrmm_channels::neural_denoise::NeuralDenoiseError> {
        let changed = match self.settings.has_changed() {
            Ok(changed) => changed,
            Err(_) => return Ok(false),
        };
        let planes = usize::from(channels).max(1);
        let stale = self
            .chain
            .as_ref()
            .is_none_or(|chain| chain.planes() != planes);
        if changed || stale {
            let settings = self.settings.borrow_and_update().clone();
            match &mut self.chain {
                Some(chain) => chain.configure(channels, &settings, profile)?,
                none => *none = Some(AudioChain::new(channels, &settings, profile)?),
            }
        }
        Ok(true)
    }
}

struct Worker {
    stages: Vec<Stage>,
    profile: ClickProfile,
    pcm_tx: broadcast::Sender<PcmBlock>,
    audio_tx: broadcast::Sender<AudioPacket>,
    commands: mpsc::Receiver<FxControl>,
    recorder: Option<AudioRecorderTap>,
    failed: bool,
}

impl Worker {
    fn run(mut self, mut source: broadcast::Receiver<PcmBlock>) {
        loop {
            let block = match source.blocking_recv() {
                Ok(block) => block,
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "audio FX fell behind its channel; audio dropped");
                    continue;
                }
                Err(RecvError::Closed) => return,
            };
            self.follow_commands();
            if self.idle() {
                return;
            }
            let Some(block) = self.process(block) else {
                return;
            };
            if let Some(recorder) = &self.recorder
                && !recorder.push(block.clone())
            {
                self.recorder = None;
            }
            let _ = self.pcm_tx.send(block);
        }
    }

    fn follow_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                FxControl::Record(tap) => self.recorder = Some(tap),
                FxControl::StopRecording => self.recorder = None,
            }
        }
    }

    fn idle(&self) -> bool {
        self.recorder.is_none()
            && self.audio_tx.receiver_count() == 0
            && self.pcm_tx.receiver_count() <= 1
    }

    fn process(&mut self, block: PcmBlock) -> Option<PcmBlock> {
        for stage in &mut self.stages {
            match stage.prepare(block.channels, self.profile) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => report(&mut self.failed, &error),
            }
        }
        let payload = match block.payload {
            PcmPayload::Silence(frames) => PcmPayload::Silence(frames),
            PcmPayload::Samples(samples) => {
                let mut pcm = samples.to_vec();
                for chain in self
                    .stages
                    .iter_mut()
                    .filter_map(|stage| stage.chain.as_mut())
                {
                    if let Err(error) = chain.process_audio(&mut pcm) {
                        report(&mut self.failed, &error);
                    }
                }
                PcmPayload::Samples(Arc::from(pcm))
            }
        };
        Some(PcmBlock { payload, ..block })
    }
}

fn report(failed: &mut bool, error: &sdrmm_channels::neural_denoise::NeuralDenoiseError) {
    if !*failed {
        tracing::error!(%error, "audio FX stage failed; its audio passes unprocessed");
        *failed = true;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AudioAgcMode, AudioFilterSettings};

    use super::*;

    fn tone(freq_hz: f64, amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| {
                amplitude
                    * (std::f64::consts::TAU * freq_hz * n as f64
                        / f64::from(sdrmm_channels::AUDIO_RATE))
                    .sin() as f32
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn source(tx: &broadcast::Sender<PcmBlock>) -> Result<FxSource, EngineError> {
        Ok(FxSource {
            pcm: tx.subscribe(),
            channels: 1,
            profile: ClickProfile::Discriminator,
            capacity: 64,
        })
    }

    fn route(fx: &[&str]) -> AudioRoute {
        AudioRoute {
            fx: fx.iter().map(|&node| node.to_owned()).collect(),
            ..AudioRoute::channel(0, 1)
        }
    }

    fn highpass() -> AudioProcessing {
        AudioProcessing {
            filter: AudioFilterSettings {
                enabled: true,
                low_hz: 1_000.0,
                high_hz: 3_000.0,
            },
            ..AudioProcessing::default()
        }
    }

    fn feed(tx: &broadcast::Sender<PcmBlock>, samples: &[f32], start_frame: u64) {
        tx.send(PcmBlock {
            start_frame,
            channels: 1,
            payload: PcmPayload::Samples(Arc::from(samples)),
        })
        .expect("the worker listens");
    }

    fn collect(rx: &mut broadcast::Receiver<PcmBlock>, blocks: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..blocks {
            match rx.blocking_recv().expect("a processed block").payload {
                PcmPayload::Samples(samples) => out.extend_from_slice(&samples),
                PcmPayload::Silence(frames) => out.resize(out.len() + frames, 0.0),
            }
        }
        out
    }

    #[test]
    fn a_route_runs_its_channel_audio_through_every_fx_node_in_order() {
        let (tx, _keep) = broadcast::channel(64);
        let mut hub = AudioFxHub::default();
        hub.set("a", highpass());
        hub.set(
            "b",
            AudioProcessing {
                agc: AudioAgcMode::Fast,
                ..AudioProcessing::default()
            },
        );
        let mut rx = hub
            .subscribe_pcm(&route(&["a", "b"]), || source(&tx))
            .expect("the route opens");
        let low = tone(200.0, 0.3, 4_800);
        for n in 0..20 {
            feed(&tx, &low, n * 4_800);
        }
        let out = collect(&mut rx, 20);
        assert_eq!(out.len(), 20 * 4_800);
        assert!(
            rms(&out[48_000..]) < 0.05,
            "the highpass let 200 Hz through"
        );
    }

    #[test]
    fn a_route_through_an_unknown_node_is_refused() {
        let (tx, _keep) = broadcast::channel::<PcmBlock>(8);
        let mut hub = AudioFxHub::default();
        let refused = hub.subscribe_pcm(&route(&["ghost"]), || source(&tx));
        assert!(matches!(refused, Err(EngineError::Audio(_))));
    }

    #[test]
    fn a_settings_change_reaches_a_running_route() {
        let (tx, _keep) = broadcast::channel(64);
        let mut hub = AudioFxHub::default();
        hub.set("a", AudioProcessing::default());
        let mut rx = hub
            .subscribe_pcm(&route(&["a"]), || source(&tx))
            .expect("the route opens");
        let low = tone(200.0, 0.3, 4_800);
        feed(&tx, &low, 0);
        assert!(rms(&collect(&mut rx, 1)) > 0.2);
        hub.set("a", highpass());
        for n in 1..11 {
            feed(&tx, &low, n * 4_800);
        }
        let out = collect(&mut rx, 10);
        assert!(rms(&out[24_000..]) < 0.05, "the new settings never arrived");
    }

    #[test]
    fn silence_passes_as_silence_with_its_stamp() {
        let (tx, _keep) = broadcast::channel(64);
        let mut hub = AudioFxHub::default();
        hub.set("a", highpass());
        let mut rx = hub
            .subscribe_pcm(&route(&["a"]), || source(&tx))
            .expect("the route opens");
        tx.send(PcmBlock {
            start_frame: 777,
            channels: 1,
            payload: PcmPayload::Silence(960),
        })
        .expect("the worker listens");
        let block = rx.blocking_recv().expect("a block");
        assert_eq!(block.start_frame, 777);
        assert!(matches!(block.payload, PcmPayload::Silence(960)));
    }

    #[test]
    fn removing_a_node_from_the_patch_ends_the_routes_through_it() {
        let (tx, _keep) = broadcast::channel(64);
        let mut hub = AudioFxHub::default();
        hub.set("a", AudioProcessing::default());
        let mut rx = hub
            .subscribe_pcm(&route(&["a"]), || source(&tx))
            .expect("the route opens");
        hub.retain(&HashSet::new());
        feed(&tx, &[0.1; 480], 0);
        assert!(matches!(rx.blocking_recv(), Err(RecvError::Closed)));
    }

    #[test]
    fn a_route_nobody_listens_to_stops() {
        let (tx, _keep) = broadcast::channel(64);
        let mut hub = AudioFxHub::default();
        hub.set("a", AudioProcessing::default());
        let rx = hub
            .subscribe_pcm(&route(&["a"]), || source(&tx))
            .expect("the route opens");
        drop(rx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hub.has(&route(&["a"])) {
            assert!(
                std::time::Instant::now() < deadline,
                "the idle route kept running"
            );
            let _ = tx.send(PcmBlock {
                start_frame: 0,
                channels: 1,
                payload: PcmPayload::Silence(480),
            });
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
