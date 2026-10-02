mod demod;
pub(crate) mod dfm;
pub(crate) mod fields;
pub(crate) mod imet;
pub(crate) mod m10;
pub(crate) mod m20;
pub(crate) mod meteomodem;
pub(crate) mod rs41;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderFamily, RadiosondeParams, SondeType,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};
use demod::{CHUNK, FrontEnd};
use dfm::Dfm;
use imet::Imet;
use meteomodem::{Accept, Meteomodem};
use rs41::Rs41;

pub(crate) const INPUT_RATE_HZ: f64 = demod::RATE;
const FILTER_TAPS: usize = 127;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "radiosonde".to_owned(),
    name: "Radiosonde".to_owned(),
    summary: "Weather balloon telemetry".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 20_000.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: false,
    decoder_kind: Some("radiosonde".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &RadiosondeParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &RadiosondeParams) -> Result<ChannelFilter, ChannelError> {
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(FILTER_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&RadiosondeParams, ChannelError> {
    match &settings.params {
        ChannelParams::Radiosonde(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "radiosonde channel got {} params",
            other.type_id()
        ))),
    }
}

fn runs(params: &RadiosondeParams, sonde: SondeType) -> bool {
    params.sonde.is_none_or(|fixed| fixed == sonde)
}

fn accept(params: &RadiosondeParams) -> Accept {
    Accept {
        m10: runs(params, SondeType::M10),
        m20: runs(params, SondeType::M20),
    }
}

pub struct RadiosondeChannel {
    params: RadiosondeParams,
    front: FrontEnd,
    rs41: Rs41,
    dfm: Dfm,
    meteomodem: Meteomodem,
    imet: Imet,
}

impl RadiosondeChannel {
    #[must_use]
    pub fn rejected(&self, sonde: SondeType) -> u32 {
        match sonde {
            SondeType::Rs41 => self.rs41.rejected(),
            SondeType::Dfm => self.dfm.rejected(),
            SondeType::M10 | SondeType::M20 => self.meteomodem.rejected(),
            SondeType::Imet4 => self.imet.rejected(),
        }
    }
}

impl ChannelRx for RadiosondeChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = *params(&settings)?;
        Ok(Self {
            params,
            front: FrontEnd::new(),
            rs41: Rs41::new(),
            dfm: Dfm::new(),
            meteomodem: Meteomodem::new(accept(&params)),
            imet: Imet::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.params = *params(&settings)?;
        self.meteomodem.set_accept(accept(&self.params));
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let meteomodem = runs(&self.params, SondeType::M10) || runs(&self.params, SondeType::M20);
        for chunk in iq.chunks(CHUNK) {
            let audio = self.front.demodulate(chunk);
            if runs(&self.params, SondeType::Rs41) {
                self.rs41.push(audio, &mut out.events);
            }
            if runs(&self.params, SondeType::Dfm) {
                self.dfm.push(audio, &mut out.events);
            }
            if meteomodem {
                self.meteomodem.push(audio, &mut out.events);
            }
            if runs(&self.params, SondeType::Imet4) {
                self.imet.push(audio, &mut out.events);
            }
        }
    }
}
