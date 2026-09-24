use std::fmt::Write;

use serde::Serialize;

use super::symbol::ERASURE;

const POSITION_ERROR: &str = "--error--";
const NOT_IMPLEMENTED: &str = "--not implemented--";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    DistressAlert,
    AllShipsCall,
    GroupCall,
    IndividualStationCall,
    GeographicAreaGroupCall,
    AutomaticServiceCall,
    Unknown,
}

impl Format {
    pub fn from_symbol(symbol: i32) -> Self {
        match symbol {
            112 => Self::DistressAlert,
            116 => Self::AllShipsCall,
            114 => Self::GroupCall,
            120 => Self::IndividualStationCall,
            102 => Self::GeographicAreaGroupCall,
            123 => Self::AutomaticServiceCall,
            _ => Self::Unknown,
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::DistressAlert => "distress_alert",
            Self::AllShipsCall => "all_ships_call",
            Self::GroupCall => "group_call",
            Self::IndividualStationCall => "individual_station_call",
            Self::GeographicAreaGroupCall => "geographic_area_group_call",
            Self::AutomaticServiceCall => "automatic_service_call",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Routine,
    Safety,
    Urgency,
    Distress,
    Unknown,
}

impl Category {
    pub fn from_symbol(symbol: i32) -> Self {
        match symbol {
            100 => Self::Routine,
            108 => Self::Safety,
            110 => Self::Urgency,
            112 => Self::Distress,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NatureOfDistress {
    FireExplosion,
    Flooding,
    Collision,
    Grounding,
    ListingInDangerOfCapsizing,
    Sinking,
    DisabledAndAdrift,
    UndesignatedDistress,
    AbandoningShip,
    PiracyArmedRobberyAttack,
    ManOverboard,
    Unknown,
}

impl NatureOfDistress {
    pub fn from_symbol(symbol: i32) -> Self {
        match symbol {
            100 => Self::FireExplosion,
            101 => Self::Flooding,
            102 => Self::Collision,
            103 => Self::Grounding,
            104 => Self::ListingInDangerOfCapsizing,
            105 => Self::Sinking,
            106 => Self::DisabledAndAdrift,
            107 => Self::UndesignatedDistress,
            108 => Self::AbandoningShip,
            109 => Self::PiracyArmedRobberyAttack,
            110 => Self::ManOverboard,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstCommand {
    AllModesTp,
    DuplexTp,
    Polling,
    UnableToComply,
    EndOfCall,
    Data,
    J3eTp,
    DistressAcknowledgement,
    DistressAlertRelay,
    TtyFec,
    TtyArq,
    Test,
    ShipPositionOrLocationRegistrationUpdating,
    NoInformation,
    Unknown,
}

impl FirstCommand {
    pub fn from_symbol(symbol: i32) -> Self {
        match symbol {
            100 => Self::AllModesTp,
            101 => Self::DuplexTp,
            103 => Self::Polling,
            104 => Self::UnableToComply,
            105 => Self::EndOfCall,
            106 => Self::Data,
            109 => Self::J3eTp,
            110 => Self::DistressAcknowledgement,
            112 => Self::DistressAlertRelay,
            113 => Self::TtyFec,
            115 => Self::TtyArq,
            118 => Self::Test,
            121 => Self::ShipPositionOrLocationRegistrationUpdating,
            126 => Self::NoInformation,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecondCommand {
    NoReasonGiven,
    CongestionAtMaritimeSwitchingCentre,
    Busy,
    QueueIndication,
    StationBarred,
    NoOperatorAvailable,
    OperatorTemporarilyUnavailable,
    EquipmentDisabled,
    UnableToUseProposedChannel,
    UnableToUseProposedMode,
    ShipsAndAircraftOfStatesNotPartiesToAnArmedConflict,
    MedicalTransports,
    PayPhonePublicCallOffice,
    FacsimileData,
    NoRemainingAcsSequentialTransmission,
    OneTimeRemainingAcsSequentialTransmission,
    TwoTimesRemainingAcsSequentialTransmission,
    ThreeTimesRemainingAcsSequentialTransmission,
    FourTimesRemainingAcsSequentialTransmission,
    FiveTimesRemainingAcsSequentialTransmission,
    NoInformation,
    Unknown,
}

impl SecondCommand {
    pub fn from_symbol(symbol: i32) -> Self {
        match symbol {
            100 => Self::NoReasonGiven,
            101 => Self::CongestionAtMaritimeSwitchingCentre,
            102 => Self::Busy,
            103 => Self::QueueIndication,
            104 => Self::StationBarred,
            105 => Self::NoOperatorAvailable,
            106 => Self::OperatorTemporarilyUnavailable,
            107 => Self::EquipmentDisabled,
            108 => Self::UnableToUseProposedChannel,
            109 => Self::UnableToUseProposedMode,
            110 => Self::ShipsAndAircraftOfStatesNotPartiesToAnArmedConflict,
            111 => Self::MedicalTransports,
            112 => Self::PayPhonePublicCallOffice,
            113 => Self::FacsimileData,
            120 => Self::NoRemainingAcsSequentialTransmission,
            121 => Self::OneTimeRemainingAcsSequentialTransmission,
            122 => Self::TwoTimesRemainingAcsSequentialTransmission,
            123 => Self::ThreeTimesRemainingAcsSequentialTransmission,
            124 => Self::FourTimesRemainingAcsSequentialTransmission,
            125 => Self::FiveTimesRemainingAcsSequentialTransmission,
            126 => Self::NoInformation,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndOfSequence {
    AcknowledgeRq,
    AcknowledgeBq,
    OtherCalls,
    Unknown,
}

impl EndOfSequence {
    pub fn from_symbol(symbol: i32) -> Self {
        match symbol {
            117 => Self::AcknowledgeRq,
            122 => Self::AcknowledgeBq,
            127 => Self::OtherCalls,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DscMessage {
    pub symbols: Vec<i32>,
    pub format: Format,
    pub category: Category,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tc1: Option<FirstCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tc2: Option<SecondCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nature: Option<NatureOfDistress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nature_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<String>,
    pub eos: EndOfSequence,
    pub ecc: i32,
    pub status: String,
}

impl DscMessage {
    fn blank(symbols: &[i32], format: Format) -> Self {
        Self {
            symbols: symbols.to_vec(),
            format,
            category: Category::Unknown,
            to: None,
            from: None,
            tc1: None,
            tc2: None,
            nature: None,
            nature_description: None,
            position: None,
            time: None,
            frequency: None,
            eos: EndOfSequence::Unknown,
            ecc: ERASURE,
            status: String::new(),
        }
    }

    fn with_tail(mut self, symbols: &[i32], eos_at: usize) -> Self {
        let ecc_at = eos_at + 1;
        self.eos = extract_eos(symbols, eos_at);
        self.ecc = at(symbols, ecc_at).unwrap_or(ERASURE);
        self.status = if ecc_valid(symbols, ecc_at) {
            "OK"
        } else {
            "Error"
        }
        .to_owned();
        self
    }

    pub fn ecc_ok(&self) -> bool {
        self.status == "OK"
    }

    pub fn is_complete(&self) -> bool {
        self.eos != EndOfSequence::Unknown && self.ecc != ERASURE
    }
}

pub fn decode(symbols: &[i32]) -> DscMessage {
    let format_symbol = match symbols.first().copied() {
        Some(symbol) if symbol != ERASURE => symbol,
        _ => symbols.get(1).copied().unwrap_or(ERASURE),
    };
    let format = Format::from_symbol(format_symbol);
    match format {
        Format::DistressAlert => decode_distress(symbols),
        Format::AllShipsCall => decode_all_ships(symbols),
        Format::IndividualStationCall => decode_individual(symbols),
        Format::GeographicAreaGroupCall => decode_area(symbols),
        Format::GroupCall | Format::AutomaticServiceCall | Format::Unknown => {
            let mut message = DscMessage::blank(symbols, format);
            if format != Format::Unknown {
                message.from = extract_mmsi(symbols, 2);
            }
            message.status = "Unsupported".to_owned();
            message
        }
    }
}

fn decode_distress(symbols: &[i32]) -> DscMessage {
    DscMessage {
        category: Category::Distress,
        to: Some("ALL SHIPS".to_owned()),
        from: extract_mmsi(symbols, 2),
        nature: at(symbols, 7).map(NatureOfDistress::from_symbol),
        position: extract_position(symbols, 8),
        time: extract_time(symbols, 13),
        ..DscMessage::blank(symbols, Format::DistressAlert)
    }
    .with_tail(symbols, 16)
}

fn decode_all_ships(symbols: &[i32]) -> DscMessage {
    let tc1 = at(symbols, 8).map(FirstCommand::from_symbol);
    DscMessage {
        category: category_at(symbols, 2),
        to: Some("ALL SHIPS".to_owned()),
        from: extract_mmsi(symbols, 3),
        tc1,
        tc2: at(symbols, 9).map(SecondCommand::from_symbol),
        frequency: (tc1 == Some(FirstCommand::J3eTp))
            .then(|| extract_frequencies(symbols, 10))
            .flatten(),
        ..DscMessage::blank(symbols, Format::AllShipsCall)
    }
    .with_tail(symbols, 16)
}

fn decode_individual(symbols: &[i32]) -> DscMessage {
    let mut message = DscMessage {
        category: category_at(symbols, 7),
        to: extract_mmsi(symbols, 2),
        from: extract_mmsi(symbols, 8),
        tc1: at(symbols, 13).map(FirstCommand::from_symbol),
        tc2: at(symbols, 14).map(SecondCommand::from_symbol),
        ..DscMessage::blank(symbols, Format::IndividualStationCall)
    };
    match at(symbols, 15) {
        Some(55) => message.position = extract_position(symbols, 16),
        Some(126) => message.nature_description = Some("Position Requested".to_owned()),
        _ => message.frequency = extract_frequencies(symbols, 15),
    }
    message.with_tail(symbols, 21)
}

fn decode_area(symbols: &[i32]) -> DscMessage {
    let tc1 = at(symbols, 13).map(FirstCommand::from_symbol);
    DscMessage {
        category: category_at(symbols, 7),
        to: extract_geographic_area(symbols, 2),
        from: extract_mmsi(symbols, 8),
        tc1,
        tc2: at(symbols, 14).map(SecondCommand::from_symbol),
        frequency: (tc1 == Some(FirstCommand::J3eTp))
            .then(|| extract_frequencies(symbols, 15))
            .flatten(),
        ..DscMessage::blank(symbols, Format::GeographicAreaGroupCall)
    }
    .with_tail(symbols, 21)
}

fn at(symbols: &[i32], index: usize) -> Option<i32> {
    symbols.get(index).copied()
}

fn category_at(symbols: &[i32], index: usize) -> Category {
    at(symbols, index).map_or(Category::Unknown, Category::from_symbol)
}

fn two_digits(out: &mut String, symbol: i32) {
    if symbol == ERASURE {
        out.push_str("__");
    } else {
        let _ = write!(out, "{symbol:02}");
    }
}

fn digits_of(symbols: &[i32]) -> String {
    let mut digits = String::with_capacity(symbols.len() * 2);
    for &symbol in symbols {
        two_digits(&mut digits, symbol);
    }
    digits
}

fn extract_mmsi(symbols: &[i32], start: usize) -> Option<String> {
    let mut mmsi = digits_of(symbols.get(start..start + 5)?);
    mmsi.pop();
    Some(mmsi)
}

fn complete_digits(symbols: &[i32], start: usize) -> Option<Result<String, ()>> {
    let slice = symbols.get(start..start + 5)?;
    let digits = digits_of(slice);
    Some(if slice.contains(&ERASURE) || digits.len() != 10 {
        Err(())
    } else {
        Ok(digits)
    })
}

fn quadrant(digits: &str) -> u8 {
    digits.get(0..1).and_then(|d| d.parse().ok()).unwrap_or(9)
}

fn extract_position(symbols: &[i32], start: usize) -> Option<String> {
    let Ok(digits) = complete_digits(symbols, start)? else {
        return Some(POSITION_ERROR.to_owned());
    };
    let (ns, ew) = match quadrant(&digits) {
        0 => ('N', 'E'),
        1 => ('N', 'W'),
        2 => ('S', 'E'),
        3 => ('S', 'W'),
        _ => return Some(POSITION_ERROR.to_owned()),
    };
    Some(format!(
        "{} {}{ns} {} {}{ew}",
        &digits[1..3],
        &digits[3..5],
        &digits[5..8],
        &digits[8..10]
    ))
}

fn extract_geographic_area(symbols: &[i32], start: usize) -> Option<String> {
    let Ok(digits) = complete_digits(symbols, start)? else {
        return Some(POSITION_ERROR.to_owned());
    };
    let quadrant_name = match quadrant(&digits) {
        0 => "North-East (NE)",
        1 => "North-West (NW)",
        2 => "South-East (SE)",
        3 => "South-West (SW)",
        _ => return Some(POSITION_ERROR.to_owned()),
    };
    let lat = digits[1..3].parse::<u32>().ok()?;
    let lon = digits[3..6].parse::<u32>().ok()?;
    let vertical = digits[6..8].parse::<u32>().ok()?;
    let horizontal = digits[8..10].parse::<u32>().ok()?;
    Some(format!(
        "{quadrant_name}, Reference point: {lat}°, {lon}°, Vertical side: {vertical}°, Horizontal side: {horizontal}°"
    ))
}

fn extract_time(symbols: &[i32], start: usize) -> Option<String> {
    let hours = at(symbols, start)?;
    let minutes = at(symbols, start + 1)?;
    if hours == ERASURE || minutes == ERASURE || hours > 23 || minutes > 59 {
        return None;
    }
    Some(format!("{hours:02}:{minutes:02}"))
}

fn extract_eos(symbols: &[i32], start: usize) -> EndOfSequence {
    let symbol = [start, start + 2, start + 3]
        .into_iter()
        .map(|index| at(symbols, index).unwrap_or(ERASURE))
        .find(|&symbol| symbol != ERASURE)
        .unwrap_or(ERASURE);
    EndOfSequence::from_symbol(symbol)
}

fn extract_frequencies(symbols: &[i32], start: usize) -> Option<String> {
    let slice = symbols.get(start..start + 6)?;
    let digits = digits_of(slice);
    let leading = digits.as_bytes().first().copied()?;
    Some(match leading {
        b'0' | b'1' | b'2' => mf_hf_100hz(slice, &digits),
        b'9' if digits.as_bytes().get(1) == Some(&b'0') => vhf_channels(&digits),
        b'3' | b'4' | b'8' => NOT_IMPLEMENTED.to_owned(),
        _ => POSITION_ERROR.to_owned(),
    })
}

fn mf_hf_100hz(slice: &[i32], digits: &str) -> String {
    if digits.len() < 12 {
        return POSITION_ERROR.to_owned();
    }
    let first = format!("{}.{}", &digits[0..5], &digits[5..6]);
    if slice[3..].iter().all(|&symbol| symbol > 99) {
        first
    } else {
        format!("{first}/{}.{}", &digits[6..11], &digits[11..12])
    }
}

fn vhf_channels(digits: &str) -> String {
    if digits.len() < 12 || digits.contains('_') {
        return POSITION_ERROR.to_owned();
    }
    let bytes = digits.as_bytes();
    let channel_type = |code: u8| match code {
        b'1' | b'2' => "Simplex channel ",
        b'0' => "Duplex channel",
        _ => "Unknown channel type",
    };
    format!(
        "{} {} - {} {}",
        channel_type(bytes[1]),
        &digits[3..6],
        channel_type(bytes[7]),
        &digits[9..12]
    )
}

pub fn ecc_valid(symbols: &[i32], ecc_at: usize) -> bool {
    let Some(ecc) = at(symbols, ecc_at).filter(|&ecc| ecc != ERASURE) else {
        return false;
    };
    let Some(information) = symbols.get(1..ecc_at.min(symbols.len())) else {
        return false;
    };
    if information.contains(&ERASURE) {
        return false;
    }
    let parity = information
        .iter()
        .fold(0, |parity, &symbol| parity ^ symbol)
        & 0x7f;
    ecc & 0x7f == parity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(symbols: &[i32]) -> DscMessage {
        decode(symbols)
    }

    #[test]
    fn format_and_category_symbol_mapping() {
        assert_eq!(Format::from_symbol(112), Format::DistressAlert);
        assert_eq!(Format::from_symbol(116), Format::AllShipsCall);
        assert_eq!(Format::from_symbol(120), Format::IndividualStationCall);
        assert_eq!(Format::from_symbol(102), Format::GeographicAreaGroupCall);
        assert_eq!(Category::from_symbol(108), Category::Safety);
        assert_eq!(
            EndOfSequence::from_symbol(122),
            EndOfSequence::AcknowledgeBq
        );
    }

    #[test]
    fn mmsi_drops_trailing_digit() {
        let symbols = [112, 112, 25, 58, 5, 99, 70];
        assert_eq!(extract_mmsi(&symbols, 2).as_deref(), Some("255805997"));
    }

    #[test]
    fn distress_alert() {
        let m = decoded(&[
            112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 52, 127, 127,
        ]);
        assert_eq!(m.format, Format::DistressAlert);
        assert_eq!(m.category, Category::Distress);
        assert_eq!(m.nature, Some(NatureOfDistress::UndesignatedDistress));
        assert_eq!(m.from.as_deref(), Some("255805997"));
        assert_eq!(m.to.as_deref(), Some("ALL SHIPS"));
        assert_eq!(m.position.as_deref(), Some("45 26N 013 07E"));
        assert_eq!(m.time.as_deref(), Some("12:52"));
        assert_eq!(m.eos, EndOfSequence::OtherCalls);
        assert_eq!(m.ecc, 52);
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn distress_alert_with_ecc_error() {
        let m = decoded(&[
            112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 51, 127, 127,
        ]);
        assert_eq!(m.from.as_deref(), Some("255805997"));
        assert_eq!(m.position.as_deref(), Some("45 26N 013 07E"));
        assert_eq!(m.ecc, 51);
        assert_eq!(m.status, "Error");
    }

    #[test]
    fn ack_safety_test_command() {
        let m = decoded(&[
            120, 120, 32, 51, 42, 0, 0, 108, 0, 23, 71, 0, 0, 118, 126, 4, 10, 10, 4, 39, 30, 122,
            54, 122, 122,
        ]);
        assert_eq!(m.format, Format::IndividualStationCall);
        assert_eq!(m.category, Category::Safety);
        assert_eq!(m.to.as_deref(), Some("325142000"));
        assert_eq!(m.from.as_deref(), Some("002371000"));
        assert_eq!(m.tc1, Some(FirstCommand::Test));
        assert_eq!(m.tc2, Some(SecondCommand::NoInformation));
        assert_eq!(m.frequency.as_deref(), Some("04101.0/04393.0"));
        assert_eq!(m.eos, EndOfSequence::AcknowledgeBq);
        assert_eq!(m.ecc, 54);
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn j3e_single_frequency() {
        let m = decoded(&[
            120, 120, 0, 23, 71, 0, 4, 100, 23, 82, 30, 0, 0, 109, 126, 8, 41, 45, 126, 126, 126,
            117, 7, 117, 117,
        ]);
        assert_eq!(m.to.as_deref(), Some("002371000"));
        assert_eq!(m.from.as_deref(), Some("238230000"));
        assert_eq!(m.frequency.as_deref(), Some("08414.5"));
        assert_eq!(m.eos, EndOfSequence::AcknowledgeRq);
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn position_requested() {
        let m = decoded(&[
            120, 120, 51, 89, 99, 19, 50, 100, 0, 27, 11, 0, 0, 126, 126, 126, 126, 126, 126, 126,
            126, 117, 81, 117, 117,
        ]);
        assert_eq!(m.to.as_deref(), Some("518999195"));
        assert_eq!(m.nature_description.as_deref(), Some("Position Requested"));
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn individual_ship_to_ship_with_position() {
        let m = decoded(&[
            120, 120, 35, 20, 2, 55, 20, 108, 27, 10, 2, 60, 10, 109, 126, 55, 3, 61, 60, 21, 23,
            117, 118, -1, -1,
        ]);
        assert_eq!(m.position.as_deref(), Some("36 16N 021 23E"));
        assert_eq!(m.eos, EndOfSequence::AcknowledgeRq);
        assert_eq!(m.ecc, 118);
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn vhf_channel_pair() {
        let m = decoded(&[
            120, 120, 37, 11, 95, 0, 0, 100, 0, 27, 11, 0, 0, 126, 126, 90, 87, 49, 90, 82, 25,
            117, 37, 117, 117,
        ]);
        assert_eq!(
            m.frequency.as_deref(),
            Some("Duplex channel 749 - Duplex channel 225")
        );
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn all_ships_safety() {
        let m = decoded(&[
            116, 116, 108, 0, 23, 71, 0, 0, 109, 126, 4, 12, 50, 4, 12, 50, 127, 36, 127, 127,
        ]);
        assert_eq!(m.format, Format::AllShipsCall);
        assert_eq!(m.category, Category::Safety);
        assert_eq!(m.from.as_deref(), Some("002371000"));
        assert_eq!(m.tc1, Some(FirstCommand::J3eTp));
        assert_eq!(m.frequency.as_deref(), Some("04125.0/04125.0"));
        assert_eq!(m.ecc, 36);
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn geographic_area_call() {
        let m = decoded(&[
            102, 102, 4, 40, 3, 5, 8, 108, 0, 22, 75, 40, 0, 109, 126, 2, 18, 20, 2, 18, 20, 127,
            49, 127, 127,
        ]);
        assert_eq!(m.format, Format::GeographicAreaGroupCall);
        assert_eq!(
            m.to.as_deref(),
            Some(
                "North-East (NE), Reference point: 44°, 3°, Vertical side: 5°, Horizontal side: 8°"
            )
        );
        assert_eq!(m.from.as_deref(), Some("002275400"));
        assert_eq!(m.frequency.as_deref(), Some("02182.0/02182.0"));
        assert_eq!(m.status, "OK");
    }

    #[test]
    fn truncated_stream_ecc_error() {
        let m = decoded(&[
            120, 120, 0, 21, 50, 10, 0, 108, 22, 93, 64, 0, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1,
        ]);
        assert_eq!(m.to.as_deref(), Some("002150100"));
        assert_eq!(m.from.as_deref(), Some("229364000"));
        assert_eq!(m.tc1, Some(FirstCommand::Unknown));
        assert_eq!(m.frequency.as_deref(), Some("--error--"));
        assert_eq!(m.eos, EndOfSequence::Unknown);
        assert_eq!(m.ecc, -1);
        assert_eq!(m.status, "Error");
        assert!(!m.is_complete());
    }

    #[test]
    fn partial_recovery_tc2_known() {
        let m = decoded(&[
            120, 120, 0, 22, 41, 2, 20, 108, 25, 75, 30, 0, 0, -1, 111, -1, -1, 126, 126, -1, -1,
            -1, -1, 117, -1,
        ]);
        assert_eq!(m.tc1, Some(FirstCommand::Unknown));
        assert_eq!(m.tc2, Some(SecondCommand::MedicalTransports));
        assert_eq!(m.frequency.as_deref(), Some("--error--"));
        assert_eq!(m.eos, EndOfSequence::AcknowledgeRq);
        assert_eq!(m.status, "Error");
    }

    #[test]
    fn mmsi_with_erasures_shows_placeholders() {
        let m = decoded(&[
            120, 120, 24, 91, -1, -1, 0, 108, -1, -1, -1, -1, -1, 100, -1, 126, 126, 126, 126, -1,
            126, 122, 4, 122, 122,
        ]);
        assert_eq!(m.to.as_deref(), Some("2491____0"));
        assert_eq!(m.from.as_deref(), Some("_________"));
        assert_eq!(m.tc1, Some(FirstCommand::AllModesTp));
        assert_eq!(m.status, "Error");
    }

    #[test]
    fn unsupported_formats_surface_the_sender() {
        let m = decoded(&[114, 114, 1, 23, 45, 67, 89]);
        assert_eq!(m.format, Format::GroupCall);
        assert_eq!(m.from.as_deref(), Some("012345678"));
        assert_eq!(m.status, "Unsupported");
        assert_eq!(decoded(&[]).format, Format::Unknown);
    }

    #[test]
    fn distress_alert_json() {
        let m = decoded(&[
            112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 52, 127, 127,
        ]);
        let v = serde_json::to_value(&m).expect("serializes");
        assert_eq!(v["format"], "distress_alert");
        assert_eq!(v["category"], "distress");
        assert_eq!(v["from"], "255805997");
        assert_eq!(v["nature"], "undesignated_distress");
        assert_eq!(v["eos"], "other_calls");
        assert_eq!(v["ecc"], 52);
        assert_eq!(v["status"], "OK");
        assert!(v.get("tc1").is_none());
    }
}
