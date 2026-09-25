use sdrmm_wire::channel::{
    AeroChannel, AisChannel, AprsMode, AtvColor, AtvModulation, AtvStandard, DabMode,
    DabTransmissionMode, DatvCodeRate, DatvRollOff, DatvStandard, DectBand, DectSides, DmrSlots,
    DrmMode, DvbtBandwidth, DvbtStandard, IlsComponent, IridiumSpan, NfmScramblerMode, NfmToneMode,
    NxdnBandwidth, PocsagBaud, PskBaud, RadioClockStandard, RttyStopBits, SelcallSystem, Sideband,
    SstvMode, SubghzModulation,
};

use super::settings::SquelchMode;

pub type Options<T> = &'static [(T, &'static str)];

pub const SQUELCH_MODES: Options<SquelchMode> = &[
    (SquelchMode::Off, "Off"),
    (SquelchMode::Manual, "Manual"),
    (SquelchMode::Auto, "Auto"),
];
pub const DMR_SLOTS: Options<DmrSlots> = &[
    (DmrSlots::Both, "Both"),
    (DmrSlots::One, "TS1"),
    (DmrSlots::Two, "TS2"),
];
pub const NXDN_WIDTHS: Options<NxdnBandwidth> = &[
    (NxdnBandwidth::Narrow, "6.25"),
    (NxdnBandwidth::Wide, "12.5"),
];
pub const DECT_BANDS: Options<DectBand> = &[(DectBand::Eu, "EU"), (DectBand::Us, "US")];
pub const DECT_SIDES: Options<DectSides> = &[
    (DectSides::Both, "Both"),
    (DectSides::Rfp, "Base"),
    (DectSides::Pp, "Handset"),
];
pub const SIDEBANDS: Options<Sideband> = &[(Sideband::Usb, "USB"), (Sideband::Lsb, "LSB")];
pub const SELCALL_SYSTEMS: Options<SelcallSystem> = &[
    (SelcallSystem::Ccir1, "CCIR-1"),
    (SelcallSystem::Zvei1, "ZVEI-1"),
];
pub const ILS_COMPONENTS: Options<IlsComponent> = &[
    (IlsComponent::Localizer, "Localizer"),
    (IlsComponent::Glideslope, "Glideslope"),
];
pub const POCSAG_BAUDS: Options<PocsagBaud> = &[
    (PocsagBaud::Auto, "Auto"),
    (PocsagBaud::B512, "512"),
    (PocsagBaud::B1200, "1200"),
    (PocsagBaud::B2400, "2400"),
];
pub const AIS_CHANNELS: Options<AisChannel> = &[(AisChannel::A, "A"), (AisChannel::B, "B")];
pub const AERO_CHANNELS: Options<AeroChannel> = &[
    (AeroChannel::P, "P"),
    (AeroChannel::Burst, "R/T"),
    (AeroChannel::C, "C"),
];
pub const IRIDIUM_SPANS: Options<IridiumSpan> = &[
    (IridiumSpan::Channel, "50 kHz"),
    (IridiumSpan::Mhz1, "1 MHz"),
    (IridiumSpan::Mhz2_5, "2.5 MHz"),
    (IridiumSpan::Mhz5, "5 MHz"),
    (IridiumSpan::Mhz10, "10 MHz"),
];
pub const APRS_MODES: Options<AprsMode> = &[
    (AprsMode::Afsk1200, "AFSK 1200"),
    (AprsMode::G3ruh9600, "G3RUH 9600"),
];
pub const NFM_TONE_MODES: Options<NfmToneMode> = &[
    (NfmToneMode::Off, "Off"),
    (NfmToneMode::Detect, "Detect"),
    (NfmToneMode::Ctcss, "CTCSS"),
    (NfmToneMode::Dcs, "DCS"),
];
pub const NFM_SCRAMBLER_MODES: Options<NfmScramblerMode> = &[
    (NfmScramblerMode::Off, "Off"),
    (NfmScramblerMode::Inversion, "Inversion"),
    (NfmScramblerMode::Auto, "Auto"),
];
pub const CTCSS_TONES_HZ: [f64; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
    110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2,
    165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5,
    206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];
pub const DCS_CODES: [u16; 83] = [
    23, 25, 26, 31, 32, 43, 47, 51, 54, 65, 71, 72, 73, 74, 114, 115, 116, 125, 131, 132, 134, 143,
    152, 155, 156, 162, 165, 172, 174, 205, 223, 226, 243, 244, 245, 251, 261, 263, 265, 271, 306,
    311, 315, 331, 343, 346, 351, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 464, 465, 466,
    503, 506, 516, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712, 723, 731,
    732, 734, 743, 754,
];
pub const CTCSS_DEFAULT_HZ: f64 = 88.5;
pub const INVERSION_DEFAULT_HZ: f64 = 3_300.0;
pub const DCS_DEFAULT_CODE: u16 = 23;
pub const RTTY_STOP_BITS: Options<RttyStopBits> = &[
    (RttyStopBits::One, "1"),
    (RttyStopBits::OneAndHalf, "1.5"),
    (RttyStopBits::Two, "2"),
];
pub const ATV_MODULATIONS: Options<AtvModulation> =
    &[(AtvModulation::Am, "AM"), (AtvModulation::Fm, "FM")];
pub const ATV_STANDARDS: Options<AtvStandard> = &[
    (AtvStandard::Ccir625, "625 / 25"),
    (AtvStandard::Eia525, "525 / 30"),
    (AtvStandard::SystemA405, "405 / 25"),
];
pub const ATV_COLORS: Options<AtvColor> = &[
    (AtvColor::Monochrome, "Mono"),
    (AtvColor::Pal, "PAL"),
    (AtvColor::Ntsc, "NTSC"),
];
pub const DAB_MODES: Options<DabMode> = &[
    (DabMode::Auto, "Auto"),
    (DabMode::Dab, "DAB"),
    (DabMode::DabPlus, "DAB+"),
];
pub const DAB_TRANSMISSION_MODES: Options<DabTransmissionMode> = &[
    (DabTransmissionMode::I, "I"),
    (DabTransmissionMode::Ii, "II"),
    (DabTransmissionMode::Iii, "III"),
    (DabTransmissionMode::Iv, "IV"),
];
pub const DATV_STANDARDS: Options<DatvStandard> = &[
    (DatvStandard::DvbS, "DVB-S"),
    (DatvStandard::DvbS2, "DVB-S2"),
];
pub const DATV_ROLL_OFFS: Options<DatvRollOff> = &[
    (DatvRollOff::Pct35, "0.35"),
    (DatvRollOff::Pct25, "0.25"),
    (DatvRollOff::Pct20, "0.20"),
    (DatvRollOff::Pct15, "0.15"),
    (DatvRollOff::Pct10, "0.10"),
    (DatvRollOff::Pct5, "0.05"),
];
pub const DATV_CODE_RATES: Options<DatvCodeRate> = &[
    (DatvCodeRate::Auto, "Auto"),
    (DatvCodeRate::Half, "1/2"),
    (DatvCodeRate::TwoThirds, "2/3"),
    (DatvCodeRate::ThreeQuarters, "3/4"),
    (DatvCodeRate::FiveSixths, "5/6"),
    (DatvCodeRate::SevenEighths, "7/8"),
];
pub const DVBT_STANDARDS: Options<DvbtStandard> = &[
    (DvbtStandard::DvbT, "DVB-T"),
    (DvbtStandard::DvbT2, "DVB-T2"),
];
pub const DVBT_BANDWIDTHS: Options<DvbtBandwidth> = &[
    (DvbtBandwidth::Mhz1_7, "1.7 MHz"),
    (DvbtBandwidth::Mhz5, "5 MHz"),
    (DvbtBandwidth::Mhz6, "6 MHz"),
    (DvbtBandwidth::Mhz7, "7 MHz"),
    (DvbtBandwidth::Mhz8, "8 MHz"),
    (DvbtBandwidth::Mhz10, "10 MHz"),
];
pub const DRM_MODES: Options<DrmMode> = &[
    (DrmMode::Auto, "Auto"),
    (DrmMode::Drm30, "DRM30"),
    (DrmMode::DrmPlus, "DRM+"),
];
pub const DEEMPHASIS_US: Options<u32> = &[(50, "50 µs"), (75, "75 µs")];
pub const RTTY_BAUDS: &[(f64, &str)] = &[(45.45, "45.45"), (50.0, "50"), (75.0, "75")];
pub const RTTY_SHIFTS_HZ: &[(f64, &str)] = &[(170.0, "170"), (450.0, "450"), (850.0, "850")];
pub const SUBGHZ_MODULATIONS: Options<SubghzModulation> = &[
    (SubghzModulation::Ook, "OOK/ASK"),
    (SubghzModulation::Fsk, "FSK"),
];
pub const PSK_BAUDS: Options<PskBaud> = &[
    (PskBaud::Psk31, "PSK31"),
    (PskBaud::Psk63, "PSK63"),
    (PskBaud::Psk125, "PSK125"),
    (PskBaud::Psk250, "PSK250"),
];
pub const RADIO_CLOCK_STANDARDS: Options<RadioClockStandard> = &[
    (RadioClockStandard::Dcf77, "DCF77"),
    (RadioClockStandard::Wwvb, "WWVB"),
    (RadioClockStandard::Msf, "MSF"),
    (RadioClockStandard::Jjy, "JJY"),
];
pub const DRM30_BANDWIDTHS_HZ: &[f64] = &[4_500.0, 5_000.0, 9_000.0, 10_000.0, 18_000.0, 20_000.0];
pub const DRM_PLUS_BANDWIDTHS_HZ: &[f64] = &[100_000.0];

#[must_use]
pub fn sstv_modes() -> Vec<(Option<SstvMode>, String)> {
    std::iter::once((None, String::from("Follow VIS")))
        .chain(
            SstvMode::ALL
                .into_iter()
                .map(|mode| (Some(mode), mode.label().to_owned())),
        )
        .collect()
}

#[must_use]
pub fn ctcss_options() -> Vec<(u32, String)> {
    CTCSS_TONES_HZ
        .iter()
        .map(|hz| (tone_key(*hz), format!("{hz:.1} Hz")))
        .collect()
}

#[must_use]
pub fn tone_key(hz: f64) -> u32 {
    (hz * 10.0).round() as u32
}

#[must_use]
pub fn dcs_options() -> Vec<(u16, String)> {
    DCS_CODES
        .iter()
        .map(|code| (*code, format!("{code:03}")))
        .collect()
}

#[must_use]
pub fn with_current<T: PartialEq + Copy>(
    value: T,
    options: Vec<(T, String)>,
    format: impl Fn(T) -> String,
) -> Vec<(T, String)> {
    if options.iter().any(|(option, _)| *option == value) {
        return options;
    }
    let mut listed = vec![(value, format!("{} (current)", format(value)))];
    listed.extend(options);
    listed
}

#[must_use]
pub fn drm_bandwidth_for(next: DrmMode, current: DrmMode, bandwidth_hz: f64) -> f64 {
    match next {
        DrmMode::Drm30 if current == DrmMode::Drm30 => bandwidth_hz,
        DrmMode::Drm30 => 10_000.0,
        _ => 100_000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_tone_and_code_is_offered() {
        assert_eq!(ctcss_options().len(), 50);
        assert_eq!(ctcss_options()[8], (885, String::from("88.5 Hz")));
        assert_eq!(dcs_options()[0], (23, String::from("023")));
        assert_eq!(dcs_options().len(), 83);
    }

    #[test]
    fn a_value_off_the_list_is_offered_as_the_current_one() {
        let listed = vec![(12_500_u32, String::from("12.5 kHz"))];
        assert_eq!(
            with_current(12_500, listed.clone(), |v| v.to_string()),
            listed
        );
        let widened = with_current(6_250, listed, |v| v.to_string());
        assert_eq!(widened[0], (6_250, String::from("6250 (current)")));
        assert_eq!(widened.len(), 2);
    }

    #[test]
    fn switching_drm_mode_picks_a_bandwidth_that_fits_it() {
        assert_eq!(
            drm_bandwidth_for(DrmMode::Drm30, DrmMode::Auto, 100_000.0),
            10_000.0
        );
        assert_eq!(
            drm_bandwidth_for(DrmMode::Drm30, DrmMode::Drm30, 4_500.0),
            4_500.0
        );
        assert_eq!(
            drm_bandwidth_for(DrmMode::DrmPlus, DrmMode::Drm30, 4_500.0),
            100_000.0
        );
        assert_eq!(
            drm_bandwidth_for(DrmMode::Auto, DrmMode::Drm30, 4_500.0),
            100_000.0
        );
    }

    #[test]
    fn sstv_follows_the_vis_code_unless_a_mode_is_named() {
        let modes = sstv_modes();
        assert_eq!(modes[0], (None, String::from("Follow VIS")));
        assert_eq!(modes.len(), 13);
        assert_eq!(modes[12].1, "Wraase SC2-180");
    }
}
