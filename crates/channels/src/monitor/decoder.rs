use num_complex::Complex;
use sdrmm_dsp::{Ddc, RealDecimator, design_lowpass};
use sdrmm_wire::{ChannelParams, ChannelSettings, DecoderEvent, IdentSignal, Modulation, Sideband};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, audio_channels,
    channel_filter, create, descriptor_of,
};

const MIN_ANALOG_SCORE: f32 = 0.7;

pub(super) struct Decoder {
    pub(super) kind: String,
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
    frequency_hz: f64,
}

pub(super) fn choices(signal: &IdentSignal) -> Vec<String> {
    let mut choices = Vec::new();
    for candidate in &signal.candidates {
        if let Some(kind) = &candidate.type_id
            && kind != "ident"
            && descriptor_of(kind).is_some()
            && (!analog_decoder(kind) || analog(signal) == Some(kind.as_str()))
            && !choices.contains(kind)
        {
            choices.push(kind.clone());
        }
    }
    if let Some(kind) = analog(signal)
        && !choices.iter().any(|choice| choice == kind)
    {
        choices.push(kind.to_owned());
    }
    choices
}

fn analog_decoder(kind: &str) -> bool {
    matches!(kind, "am" | "nfm" | "ssb" | "wfm")
}

fn analog(signal: &IdentSignal) -> Option<&'static str> {
    let kind = match signal.modulation {
        Modulation::Am => Some("am"),
        Modulation::Fm if signal.bandwidth_hz > 50_000.0 => Some("wfm"),
        Modulation::Fm => Some("nfm"),
        Modulation::Ssb => Some("ssb"),
        _ => None,
    }?;
    signal
        .candidates
        .iter()
        .any(|candidate| {
            candidate.type_id.as_deref() == Some(kind) && candidate.score >= MIN_ANALOG_SCORE
        })
        .then_some(kind)
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
            frequency_hz: signal.frequency_hz,
        })
    }

    pub(super) fn retune(&mut self, offset_hz: f64) {
        self.ddc.set_offset(offset_hz + self.tuning_offset_hz);
    }

    pub(super) fn selected(&self, signal: &IdentSignal) -> bool {
        self.verified || analog(signal) == Some(self.kind.as_str())
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
        self.out
            .events
            .retain(|event| valid(event, self.frequency_hz));
        self.verified |= self.out.events.iter().any(verifies);
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

fn verifies(event: &DecoderEvent) -> bool {
    !matches!(
        event,
        DecoderEvent::Morse(_) | DecoderEvent::Rtty(_) | DecoderEvent::Psk(_)
    )
}

fn valid(event: &DecoderEvent, frequency_hz: f64) -> bool {
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
        DecoderEvent::Subghz(frame) => {
            frame.reading.is_some()
                || frame.encoding != sdrmm_wire::SubghzEncoding::Raw
                    && frame.bits > 0
                    && frame.repeats >= 2
        }
        DecoderEvent::Ils(reading) => {
            crate::ident::in_allocation("ils", frequency_hz)
                && reading.modulation_90 >= 0.05
                && reading.modulation_150 >= 0.05
        }
        DecoderEvent::Tone(_) | DecoderEvent::Scrambler(_) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{IlsComponent, IlsReading, MorseText, ProtocolMatch, SubghzFrame};

    use super::*;

    #[test]
    fn weak_voice_candidates_do_not_select_analog_audio() {
        let mut signal = IdentSignal {
            modulation: Modulation::Fm,
            bandwidth_hz: 7_000.0,
            confidence: 0.97,
            candidates: vec![ProtocolMatch {
                name: "FM voice (narrowband)".to_owned(),
                type_id: Some("nfm".to_owned()),
                score: 0.48,
                confirmed: false,
                why: String::new(),
            }],
            ..Default::default()
        };
        assert!(choices(&signal).is_empty());
        signal.candidates[0].score = 0.95;
        assert_eq!(choices(&signal), ["nfm"]);
        signal.modulation = Modulation::Fsk4;
        assert!(choices(&signal).is_empty());
    }

    #[test]
    fn raw_pulses_do_not_confirm_a_decoder() {
        assert!(!valid(
            &DecoderEvent::Subghz(SubghzFrame {
                repeats: 30,
                timings_us: vec![100; 31],
                ..Default::default()
            }),
            435_125_000.0
        ));
    }

    #[test]
    fn ils_measurements_outside_its_allocation_are_rejected() {
        let event = DecoderEvent::Ils(IlsReading {
            component: IlsComponent::Localizer,
            modulation_90: 0.2,
            modulation_150: 0.2,
            ddm: 0.0,
            deviation_dots: 0.0,
            signal_db: -20.0,
        });
        assert!(!valid(&event, 435_125_000.0));
        assert!(valid(&event, 110_300_000.0));
    }

    #[test]
    fn unframed_text_does_not_stop_decoder_trials() {
        assert!(!verifies(&DecoderEvent::Morse(MorseText {
            text: "**".to_owned(),
            wpm: 79.0,
        })));
    }
}
