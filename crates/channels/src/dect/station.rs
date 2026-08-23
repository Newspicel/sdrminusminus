use sdrmm_wire::{
    DectBand, DectCapability, DectCipherState, DectFrame, DectIdentity, DectSecurity, DectSide,
    DectUpdate,
};

use super::{
    burst::{Burst, FRAME_SAMPLES, INPUT_RATE_HZ},
    identity::Rfpi,
    mac::{self, EncryptionCommand, EncryptionPhase, Header, StaticInfo, Tail, a_field_crc_ok},
};

const MAX_TRACKS: usize = 24;
const SLOT_TOLERANCE: u64 = 200;
const MAX_HANDSETS: usize = 16;
const REFRESH_SAMPLES: u64 = (INPUT_RATE_HZ * 5.0) as u64;

fn set<T: PartialEq>(slot: &mut T, value: T, dirty: &mut bool) {
    if *slot != value {
        *slot = value;
        *dirty = true;
    }
}

#[derive(Default)]
struct Track {
    used: bool,
    anchor: u64,
    last: u64,
    last_emit: u64,
    emitted: bool,
    side: DectSide,
    identity: Option<DectIdentity>,
    carrier: Option<u8>,
    slot_pair: Option<u8>,
    rf_carriers: Option<u16>,
    transceivers: Option<u8>,
    pscn: Option<u8>,
    extended_carriers: bool,
    multiframe: Option<u32>,
    capabilities: Vec<DectCapability>,
    security: DectSecurity,
    fmid: Option<u16>,
    pmid: Option<u32>,
    handsets: Vec<u32>,
    bursts: u32,
    crc_errors: u32,
    pending_errors: bool,
    level_dbfs: f32,
}

impl Track {
    fn reset(&mut self, sample: u64, side: DectSide) {
        *self = Self {
            used: true,
            anchor: sample,
            last: sample,
            side,
            ..Self::default()
        };
    }

    fn matches(&self, sample: u64, side: DectSide) -> bool {
        if !self.used || self.side != side {
            return false;
        }
        let delta = sample.wrapping_sub(self.anchor) % FRAME_SAMPLES;
        delta.min(FRAME_SAMPLES - delta) <= SLOT_TOLERANCE
    }

    fn note_handset(&mut self, pmid: u32, dirty: &mut bool) {
        if self.handsets.contains(&pmid) {
            return;
        }
        if self.handsets.len() == MAX_HANDSETS {
            self.handsets.remove(0);
        }
        self.handsets.push(pmid);
        *dirty = true;
    }

    fn apply_static(&mut self, info: StaticInfo, band: DectBand, dirty: &mut bool) {
        set(&mut self.slot_pair, Some(info.slot_pair), dirty);
        set(&mut self.transceivers, Some(info.transceivers + 1), dirty);
        set(&mut self.rf_carriers, Some(info.rf_carriers), dirty);
        set(&mut self.pscn, Some(info.pscn), dirty);
        set(&mut self.extended_carriers, info.extended_carriers, dirty);
        if band.carrier_hz(info.carrier).is_some() {
            set(&mut self.carrier, Some(info.carrier), dirty);
        }
    }

    fn apply_capabilities(&mut self, a_field: u64, dirty: &mut bool) {
        let mut found = Vec::new();
        mac::capabilities(a_field, &mut found);
        let authentication = found.contains(&DectCapability::StandardAuthentication);
        let ciphering = found.contains(&DectCapability::StandardCiphering);
        if self.capabilities != found {
            self.capabilities = found;
            *dirty = true;
        }
        set(
            &mut self.security.authentication_supported,
            Some(authentication),
            dirty,
        );
        set(
            &mut self.security.ciphering_supported,
            Some(ciphering),
            dirty,
        );
    }

    fn apply_encryption(&mut self, a_field: u64, dirty: &mut bool) {
        let message = mac::encryption(a_field);
        let state = match (message.command, message.phase) {
            (EncryptionCommand::Stop, _) => DectCipherState::Stopped,
            (EncryptionCommand::Reserved, _) => return,
            (_, EncryptionPhase::Request) => DectCipherState::Requested,
            (_, EncryptionPhase::Confirm) => DectCipherState::Confirmed,
            (_, EncryptionPhase::Grant) => DectCipherState::Active,
            (_, EncryptionPhase::Reject) => DectCipherState::Clear,
        };
        let command = match message.command {
            EncryptionCommand::Start => "start encryption",
            EncryptionCommand::Stop => "stop encryption",
            EncryptionCommand::StartWithKeyIndex => "start encryption with key index",
            EncryptionCommand::Reserved => "reserved",
        };
        let phase = match message.phase {
            EncryptionPhase::Request => "request",
            EncryptionPhase::Confirm => "confirm",
            EncryptionPhase::Grant => "grant",
            EncryptionPhase::Reject => "reject",
        };
        set(&mut self.security.cipher_state, state, dirty);
        set(
            &mut self.security.last_command,
            Some(format!("{command}: {phase}")),
            dirty,
        );
        if let Some(index) = message.key_index {
            set(&mut self.security.cipher_key_index, Some(index), dirty);
        }
        if let Some(fmid) = message.fmid {
            set(&mut self.fmid, Some(fmid), dirty);
        }
        if let Some(pmid) = message.pmid {
            set(&mut self.pmid, Some(pmid), dirty);
            self.note_handset(pmid, dirty);
        }
        self.security.encryption_events += 1;
        *dirty = true;
    }

    fn frame(&self, update: DectUpdate, band: DectBand) -> DectFrame {
        DectFrame {
            side: self.side,
            update,
            identity: self.identity.clone(),
            carrier: self.carrier,
            carrier_hz: self.carrier.and_then(|c| band.carrier_hz(c)),
            slot_pair: self.slot_pair,
            rf_carriers: self.rf_carriers,
            transceivers: self.transceivers,
            pscn: self.pscn,
            extended_carriers: self.extended_carriers,
            multiframe: self.multiframe,
            security: self.security.clone(),
            capabilities: self.capabilities.clone(),
            fmid: self.fmid,
            pmid: self.pmid,
            handsets: self.handsets.clone(),
            bursts: self.bursts,
            crc_errors: self.crc_errors,
            level_dbfs: self.level_dbfs,
        }
    }
}

pub(crate) struct Tracker {
    tracks: Vec<Track>,
    band: DectBand,
}

impl Tracker {
    pub fn new(band: DectBand) -> Self {
        let mut tracks = Vec::with_capacity(MAX_TRACKS);
        tracks.resize_with(MAX_TRACKS, Track::default);
        Self { tracks, band }
    }

    pub fn set_band(&mut self, band: DectBand) {
        self.band = band;
    }

    pub fn clear(&mut self) {
        for track in &mut self.tracks {
            track.used = false;
        }
    }

    fn index_for(&mut self, sample: u64, side: DectSide) -> usize {
        if let Some(index) = self
            .tracks
            .iter()
            .position(|track| track.matches(sample, side))
        {
            return index;
        }
        let index = self
            .tracks
            .iter()
            .position(|track| !track.used)
            .unwrap_or_else(|| {
                self.tracks
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, track)| track.last)
                    .map_or(0, |(index, _)| index)
            });
        self.tracks[index].reset(sample, side);
        index
    }

    pub fn apply(&mut self, burst: &Burst) -> Option<DectFrame> {
        let side = if burst.from_rfp {
            DectSide::Rfp
        } else {
            DectSide::Pp
        };
        let index = self.index_for(burst.sample, side);
        let band = self.band;
        let track = &mut self.tracks[index];
        track.anchor = burst.sample;
        track.last = burst.sample;
        track.level_dbfs = burst.level_dbfs;

        if !a_field_crc_ok(burst.a_field) {
            track.crc_errors = track.crc_errors.saturating_add(1);
            track.pending_errors = true;
            return None;
        }
        track.bursts = track.bursts.saturating_add(1);

        let mut dirty = !track.emitted || track.pending_errors;
        track.pending_errors = false;
        let Header { tail, .. } = mac::header(burst.a_field, burst.from_rfp);
        let update = match tail {
            Tail::Nt | Tail::NtConnectionless => {
                let identity = Rfpi::parse(mac::rfpi(burst.a_field)).describe();
                set(&mut track.identity, Some(identity), &mut dirty);
                DectUpdate::Identity
            }
            Tail::Qt => match mac::qt_head(burst.a_field) {
                0 | 1 => {
                    track.apply_static(mac::static_info(burst.a_field), band, &mut dirty);
                    DectUpdate::SystemInfo
                }
                3 => {
                    track.apply_capabilities(burst.a_field, &mut dirty);
                    DectUpdate::Capabilities
                }
                6 => {
                    let multiframe = mac::multiframe_number(burst.a_field);
                    set(&mut track.multiframe, Some(multiframe), &mut dirty);
                    DectUpdate::SystemInfo
                }
                _ => DectUpdate::SystemInfo,
            },
            Tail::Mt | Tail::MtFirst => {
                if mac::mt_head(burst.a_field) == 5 {
                    track.apply_encryption(burst.a_field, &mut dirty);
                    DectUpdate::Encryption
                } else {
                    DectUpdate::Bearer
                }
            }
            Tail::Pt => DectUpdate::Paging,
            Tail::Ct { .. } | Tail::Escape => DectUpdate::Bearer,
        };

        let stale = burst.sample.saturating_sub(track.last_emit) >= REFRESH_SAMPLES;
        if !dirty && !stale {
            return None;
        }
        track.last_emit = burst.sample;
        track.emitted = true;
        Some(track.frame(update, band))
    }
}
