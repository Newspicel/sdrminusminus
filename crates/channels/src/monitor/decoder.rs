use num_complex::Complex;
use sdrmm_dsp::{Ddc, RealDecimator, design_lowpass};
use sdrmm_wire::{ChannelParams, ChannelSettings, DecoderEvent, IdentSignal, Modulation, Sideband};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, audio_channels,
    channel_filter, create, descriptor_of,
};

pub(super) struct Decoder {
    pub(super) kind: String,
    pub(super) confirmed: bool,
    pub(super) verified: bool,
    ddc: Ddc,
    tuning_offset_hz: f64,
    filter: ChannelFilter,
    channel: Box<dyn ChannelRx>,
    tuned: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    out: ChannelOutputs,
    channels: usize,
    decimator: RealDecimator,
    mono: Vec<f32>,
    downsampled: Vec<f32>,
    pub(super) audio: Vec<i16>,
    record: bool,
}

pub(super) fn choices(signal: &IdentSignal) -> Vec<String> {
    let mut choices = Vec::new();
    for candidate in &signal.candidates {
        if let Some(kind) = &candidate.type_id
            && kind != "ident"
            && descriptor_of(kind).is_some()
            && !choices.contains(kind)
        {
            choices.push(kind.clone());
        }
    }
    let analog = match signal.modulation {
        Modulation::Am => Some("am"),
        Modulation::Fm if signal.bandwidth_hz > 50_000.0 => Some("wfm"),
        Modulation::Fm => Some("nfm"),
        Modulation::Ssb => Some("ssb"),
        _ => None,
    };
    if let Some(kind) = analog
        && !choices.iter().any(|choice| choice == kind)
    {
        choices.push(kind.to_owned());
    }
    choices
}

impl Decoder {
    pub(super) fn new(
        kind: &str,
        rate: f64,
        signal: &IdentSignal,
        record: bool,
    ) -> Result<Self, ChannelError> {
        let mut settings = ChannelSettings::default_for(kind)
            .ok_or_else(|| ChannelError::UnknownType(kind.to_owned()))?;
        settings.frequency_hz = signal.frequency_hz;
        settings.squelch = sdrmm_wire::Squelch::Off;
        let mut offset = signal.center_offset_hz;
        match &mut settings.params {
            ChannelParams::Am(p) => {
                p.bandwidth_hz = (signal.bandwidth_hz * 1.25).clamp(4000.0, 30_000.0)
            }
            ChannelParams::Nfm(p) => {
                p.bandwidth_hz = (signal.bandwidth_hz * 1.25).clamp(7500.0, 30_000.0)
            }
            ChannelParams::Ssb(p) => {
                p.sideband = signal.sideband.unwrap_or(Sideband::Usb);
                p.bandwidth_hz = signal.bandwidth_hz.clamp(1500.0, 6000.0);
                offset += match p.sideband {
                    Sideband::Usb => -p.bandwidth_hz / 2.0,
                    Sideband::Lsb => p.bandwidth_hz / 2.0,
                };
            }
            ChannelParams::Wfm(p) => p.stereo = false,
            _ => {}
        }
        let input_rate = crate::input_rate(&settings.params);
        Ok(Self {
            kind: kind.to_owned(),
            confirmed: matches!(kind, "am" | "nfm" | "ssb" | "wfm"),
            verified: false,
            tuning_offset_hz: offset - signal.center_offset_hz,
            ddc: Ddc::new(rate, input_rate, offset)
                .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?,
            filter: channel_filter(&settings.params)?,
            channels: usize::from(audio_channels(&settings.params)),
            channel: create(ChannelCtx { input_rate }, &settings)?,
            tuned: Vec::new(),
            filtered: Vec::new(),
            out: ChannelOutputs::default(),
            decimator: RealDecimator::new(&design_lowpass(96, 3400.0 / 48_000.0), 6),
            mono: Vec::new(),
            downsampled: Vec::new(),
            audio: Vec::new(),
            record,
        })
    }

    pub(super) fn retune(&mut self, offset_hz: f64) {
        self.ddc.set_offset(offset_hz + self.tuning_offset_hz);
    }

    pub(super) fn process(&mut self, iq: &[Complex<f32>]) -> (Vec<DecoderEvent>, Option<String>) {
        self.ddc.process(iq, &mut self.tuned);
        self.filter.process(&self.tuned, &mut self.filtered);
        self.out.reset();
        self.channel.process(&self.filtered, &mut self.out);
        let mut error = None;
        if !self.out.images.is_empty() || !self.out.video.is_empty() {
            error = Some("decoded images or video are not supported by event export".to_owned());
        }
        self.out.events.retain(valid);
        self.verified |= !self.out.events.is_empty();
        self.confirmed |= self.verified;
        if self.record && !self.out.audio_pcm.is_empty() {
            if self.out.audio_rate != 48_000 {
                error = Some(format!(
                    "unsupported decoder audio rate {}",
                    self.out.audio_rate
                ));
            } else {
                self.mono.clear();
                self.mono.extend(
                    self.out
                        .audio_pcm
                        .chunks_exact(self.channels)
                        .map(|frame| frame.iter().sum::<f32>() / self.channels as f32),
                );
                self.decimator.process(&self.mono, &mut self.downsampled);
                let room = (8000 * 31usize).saturating_sub(self.audio.len());
                if self.downsampled.len() > room {
                    error = Some("audio buffer limit reached".to_owned());
                }
                self.audio.extend(
                    self.downsampled
                        .iter()
                        .take(room)
                        .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0) as i16),
                );
            }
        }
        (std::mem::take(&mut self.out.events), error)
    }
}

fn valid(event: &DecoderEvent) -> bool {
    match event {
        DecoderEvent::Dsc(m)
        | DecoderEvent::Vdl2(m)
        | DecoderEvent::Hfdl(m)
        | DecoderEvent::InmarsatStdc(m)
        | DecoderEvent::InmarsatAero(m)
        | DecoderEvent::Iridium(m) => m.crc_ok,
        DecoderEvent::Dv(frame) => {
            frame.crc_verified == Some(true)
                || matches!(frame.kind, sdrmm_wire::DvFrameKind::Voice)
                    && frame.crc_verified != Some(false)
        }
        DecoderEvent::Tone(_) | DecoderEvent::Scrambler(_) => false,
        _ => true,
    }
}
