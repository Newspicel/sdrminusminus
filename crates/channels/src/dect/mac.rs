use sdrmm_wire::DectCapability;

pub(crate) const A_FIELD_BITS: usize = 64;
const R_CRC_POLY: u32 = 0x0589;
const R_CRC_RESIDUE: u16 = 0x0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tail {
    Ct { packet: u8 },
    NtConnectionless,
    Nt,
    Qt,
    Escape,
    Mt,
    Pt,
    MtFirst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub tail: Tail,
    pub ba: u8,
    pub q1: bool,
    pub q2: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StaticInfo {
    pub normal_reverse: bool,
    pub slot_pair: u8,
    pub start_position: u8,
    pub escape: bool,
    pub transceivers: u8,
    pub extended_carriers: bool,
    pub rf_carriers: u16,
    pub carrier: u8,
    pub extended_system_info: bool,
    pub pscn: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EncryptionCommand {
    Start,
    Stop,
    StartWithKeyIndex,
    Reserved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EncryptionPhase {
    Request,
    Confirm,
    Grant,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Encryption {
    pub command: EncryptionCommand,
    pub phase: EncryptionPhase,
    pub key_index: Option<u16>,
    pub fmid: Option<u16>,
    pub pmid: Option<u32>,
}

pub(crate) const CAPABILITY_BITS: [(usize, DectCapability); 34] = [
    (12, DectCapability::ExtendedFpInfo),
    (13, DectCapability::DoubleDuplexBearer),
    (15, DectCapability::DoubleSlot),
    (16, DectCapability::HalfSlot),
    (17, DectCapability::FullSlot),
    (18, DectCapability::FrequencyControl),
    (19, DectCapability::PageRepetition),
    (20, DectCapability::CoSetupOnDummy),
    (21, DectCapability::ClUplink),
    (22, DectCapability::ClDownlink),
    (23, DectCapability::BasicAFieldSetup),
    (24, DectCapability::AdvancedAFieldSetup),
    (25, DectCapability::BFieldSetup),
    (26, DectCapability::CfMessages),
    (27, DectCapability::InMinimumDelay),
    (28, DectCapability::InNormalDelay),
    (29, DectCapability::IpErrorDetection),
    (30, DectCapability::IpErrorCorrection),
    (31, DectCapability::MultibearerConnections),
    (32, DectCapability::Adpcm),
    (33, DectCapability::GapBasicSpeech),
    (34, DectCapability::NonVoiceCircuitSwitched),
    (35, DectCapability::NonVoicePacketSwitched),
    (36, DectCapability::StandardAuthentication),
    (37, DectCapability::StandardCiphering),
    (38, DectCapability::LocationRegistration),
    (39, DectCapability::SimServices),
    (40, DectCapability::NonStaticFixedPart),
    (41, DectCapability::CissServices),
    (42, DectCapability::ClmsService),
    (43, DectCapability::ComsService),
    (44, DectCapability::AccessRightsRequests),
    (45, DectCapability::ExternalHandover),
    (46, DectCapability::ConnectionHandover),
];

#[must_use]
pub(crate) fn field(a: u64, offset: usize, width: usize) -> u64 {
    debug_assert!(offset + width <= A_FIELD_BITS);
    (a >> (A_FIELD_BITS - offset - width)) & ((1u64 << width) - 1)
}

#[must_use]
pub(crate) fn bit(a: u64, offset: usize) -> bool {
    field(a, offset, 1) == 1
}

#[must_use]
pub(crate) fn r_crc(a: u64, bits: usize) -> u16 {
    let mut reg: u32 = 0;
    for index in 0..bits {
        let input = u32::from((a >> (A_FIELD_BITS - 1 - index)) & 1 != 0);
        let feedback = ((reg >> 15) & 1) ^ input;
        reg = (reg << 1) & 0xFFFF;
        if feedback == 1 {
            reg ^= R_CRC_POLY;
        }
    }
    reg as u16
}

#[must_use]
pub(crate) fn a_field_crc_ok(a: u64) -> bool {
    let expected = r_crc(a, 48) ^ R_CRC_RESIDUE;
    expected == field(a, 48, 16) as u16
}

#[cfg(any(test, feature = "test-signals"))]
#[must_use]
pub(crate) fn append_r_crc(a: u64) -> u64 {
    let head = a & !((1u64 << 16) - 1);
    head | u64::from(r_crc(head, 48) ^ R_CRC_RESIDUE)
}

#[must_use]
pub(crate) fn header(a: u64, from_rfp: bool) -> Header {
    let ta = field(a, 0, 3) as u8;
    let ba = field(a, 4, 3) as u8;
    let tail = match (ta, from_rfp) {
        (0, _) => Tail::Ct { packet: 0 },
        (1, _) => Tail::Ct { packet: 1 },
        (2, _) => Tail::NtConnectionless,
        (3, _) => Tail::Nt,
        (4, _) => Tail::Qt,
        (5, _) => Tail::Escape,
        (6, _) => Tail::Mt,
        (_, true) => Tail::Pt,
        (_, false) => Tail::MtFirst,
    };
    Header {
        tail,
        ba,
        q1: bit(a, 3),
        q2: bit(a, 7),
    }
}

#[must_use]
pub(crate) fn rfpi(a: u64) -> u64 {
    field(a, 8, 40)
}

#[must_use]
pub(crate) fn qt_head(a: u64) -> u8 {
    field(a, 8, 4) as u8
}

#[must_use]
pub(crate) fn static_info(a: u64) -> StaticInfo {
    StaticInfo {
        normal_reverse: bit(a, 11),
        slot_pair: field(a, 12, 4) as u8,
        start_position: field(a, 16, 2) as u8,
        escape: bit(a, 18),
        transceivers: field(a, 19, 2) as u8,
        extended_carriers: bit(a, 21),
        rf_carriers: field(a, 22, 10) as u16,
        carrier: field(a, 34, 6) as u8,
        extended_system_info: bit(a, 40),
        pscn: field(a, 42, 6) as u8,
    }
}

pub(crate) fn capabilities(a: u64, out: &mut Vec<DectCapability>) {
    out.clear();
    for &(offset, capability) in &CAPABILITY_BITS {
        if bit(a, offset) {
            out.push(capability);
        }
    }
}

#[must_use]
pub(crate) fn mt_head(a: u64) -> u8 {
    field(a, 8, 4) as u8
}

#[must_use]
pub(crate) fn encryption(a: u64) -> Encryption {
    let command = match field(a, 12, 2) {
        0 => EncryptionCommand::Start,
        1 => EncryptionCommand::Stop,
        2 => EncryptionCommand::StartWithKeyIndex,
        _ => EncryptionCommand::Reserved,
    };
    let phase = match field(a, 14, 2) {
        0 => EncryptionPhase::Request,
        1 => EncryptionPhase::Confirm,
        2 => EncryptionPhase::Grant,
        _ => EncryptionPhase::Reject,
    };
    let keyed = matches!(command, EncryptionCommand::StartWithKeyIndex);
    Encryption {
        command,
        phase,
        key_index: keyed.then(|| field(a, 16, 16) as u16),
        fmid: (!keyed).then(|| field(a, 16, 12) as u16),
        pmid: (!keyed).then(|| field(a, 28, 20) as u32),
    }
}

#[must_use]
pub(crate) fn multiframe_number(a: u64) -> u32 {
    field(a, 24, 24) as u32
}
