use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub mod beamformer;
pub mod correlator;
pub mod df;
pub mod polarimeter;
pub mod spatial;
pub mod stitch;

pub const MAX_AT_LEN: usize = 40;
pub const MIN_BAND_HZ: f64 = 1_000.0;

pub(crate) fn reserved_at() -> String {
    String::with_capacity(MAX_AT_LEN)
}

pub(crate) fn finite_within(value: f64, limit: f64) -> bool {
    (-limit..=limit).contains(&value)
}

pub(crate) fn power_of_two_in(value: u32, min: u32, max: u32) -> bool {
    value.is_power_of_two() && (min..=max).contains(&value)
}

macro_rules! processors {
    ($callback:ident) => {
        $callback! {
            processors: [
                (
                    Df,
                    "df",
                    $crate::processor::df::DfParams,
                    $crate::processor::df::DfReading,
                    DfNode,
                    "Direction finder",
                    "Bearings from an array",
                    [array_in, events_out("True bearings")]
                ),
                (
                    Beamformer,
                    "beamformer",
                    $crate::processor::beamformer::BeamformerParams,
                    $crate::processor::beamformer::BeamformerReading,
                    BeamformerNode,
                    "Beamformer",
                    "Steer, null or combine an array",
                    [
                        array_in,
                        steer_in("A direction finder to steer at"),
                        beam_out("The combined lane")
                    ]
                ),
                (
                    PassiveRadar,
                    "passive_radar",
                    $crate::radar::PassiveRadarParams,
                    $crate::radar::RadarUpdate,
                    PassiveRadarNode,
                    "Passive radar",
                    "Aircraft echoes of FM, DAB or DVB-T",
                    [
                        array_in,
                        tx_in("GPS node fixed at the transmitter"),
                        adsb_in("ADS-B decoder used as truth"),
                        events_out("Track events")
                    ]
                ),
                (
                    Stitch,
                    "stitch",
                    $crate::processor::stitch::StitchParams,
                    $crate::processor::stitch::StitchReading,
                    StitchNode,
                    "Stitch",
                    "Spread lanes joined into one wide band",
                    [array_in, wide_out("Every lane joined")]
                ),
                (
                    SpatialSpectrum,
                    "spatial_spectrum",
                    $crate::processor::spatial::SpatialSpectrumParams,
                    $crate::processor::spatial::SpatialReading,
                    SpatialSpectrumNode,
                    "Spatial spectrum",
                    "Bearing over frequency",
                    [array_in]
                ),
                (
                    Correlator,
                    "correlator",
                    $crate::processor::correlator::CorrelatorParams,
                    $crate::processor::correlator::CorrelatorReading,
                    CorrelatorNode,
                    "Correlator",
                    "Baseline visibilities",
                    [array_in]
                ),
                (
                    Polarimeter,
                    "polarimeter",
                    $crate::processor::polarimeter::PolarimeterParams,
                    $crate::processor::polarimeter::PolarimeterReading,
                    PolarimeterNode,
                    "Polarimeter",
                    "Polarisation of two crossed antennas",
                    [array_in, beam_out("Matched to the wave")]
                ),
            ],
            probe: (Probe, "probe", $crate::processor::ProbeParams),
        }
    };
}

pub(crate) use processors;

macro_rules! define_processor_enums {
    (
        processors: [$((
            $variant:ident,
            $type_id:literal,
            $params:ty,
            $reading:ty,
            $node:ident,
            $name:literal,
            $summary:literal,
            [$($port:tt)*]
        )),* $(,)?],
        probe: ($probe:ident, $probe_id:literal, $probe_params:ty) $(,)?
    ) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "type", content = "settings")]
        pub enum ProcessorParams {
            $(
                #[serde(rename = $type_id)]
                $variant($params),
            )*
            #[cfg(feature = "probe")]
            #[serde(rename = $probe_id)]
            $probe($probe_params),
        }

        impl ProcessorParams {
            #[must_use]
            pub const fn type_id(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $type_id,)*
                    #[cfg(feature = "probe")]
                    Self::$probe(_) => $probe_id,
                }
            }

            #[must_use]
            pub fn problem(&self) -> Option<&'static str> {
                match self {
                    $(Self::$variant(params) => params.problem(),)*
                    #[cfg(feature = "probe")]
                    Self::$probe(params) => params.problem(),
                }
            }

            #[must_use]
            pub fn valid(&self) -> bool {
                self.problem().is_none()
            }
        }

        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
        #[serde(tag = "type", content = "reading")]
        pub enum ProcessorReading {
            $(
                #[serde(rename = $type_id)]
                $variant($reading),
            )*
        }

        impl ProcessorReading {
            #[must_use]
            pub const fn type_id(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $type_id,)*
                }
            }

            #[must_use]
            pub fn empty(type_id: &str) -> Option<Self> {
                match type_id {
                    $($type_id => Some(Self::$variant(<$reading>::reserved())),)*
                    _ => None,
                }
            }
        }

        pub const PROCESSOR_TYPE_IDS: &[&str] = &[$($type_id),*];
    };
}

processors!(define_processor_enums);

#[cfg(feature = "probe")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProbeParams {
    pub time: bool,
    pub phase: bool,
    pub gain: bool,
    pub spread: bool,
    pub lane_ports: u8,
    pub rebuild: u32,
    pub block_drop_ms: u32,
}

#[cfg(feature = "probe")]
impl ProbeParams {
    #[must_use]
    pub const fn problem(&self) -> Option<&'static str> {
        None
    }
}

#[cfg(test)]
mod tests;
