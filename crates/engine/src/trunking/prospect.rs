use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use sdrmm_wire::{
    DmrChannelEntry, DmrDiscovery, DvFrame, DvFrameKind, TrunkChannel, TrunkChannelSource,
};

use super::Heard;

const GRANT_WINDOW: Duration = Duration::from_secs(8);

const SEARCH_IDLE: Duration = Duration::from_secs(60);

const DWELL: Duration = Duration::from_secs(20);

const CONFIRM_SCORE: i32 = 4;

const MAX_SCORE: i32 = 8;

const IDENTIFIED_WEIGHT: i32 = 2;

const ANONYMOUS_WEIGHT: i32 = 1;

const CONTRADICTION_WEIGHT: i32 = 1;

const MAX_PENDING: usize = 64;

const CARRIER_MEMORY: Duration = Duration::from_secs(5);

/// How many snapshots in a row a candidate has to be loud in before the search believes it. One
/// is a noise run; two is a transmitter.
const CARRIER_SIGHTINGS: u32 = 2;

const CARRIER_SNAP_HZ: u64 = 2_500;

const RASTER_HZ: u64 = 12_500;

const FIT_MINIMUM_POINTS: usize = 3;

const FIT_STEP_GRANULARITY_HZ: i64 = 1_250;

const FIT_TOLERANCE_HZ: i64 = 625;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BandPlan {
    base_hz: i64,
    step_hz: i64,
}

impl BandPlan {
    fn at(&self, logical_channel: u16) -> Option<u64> {
        let freq = self.base_hz + self.step_hz * i64::from(logical_channel);
        u64::try_from(freq).ok().filter(|freq| *freq > 0)
    }

    fn fit(points: &[(u16, u64)]) -> Option<Self> {
        if points.len() < FIT_MINIMUM_POINTS {
            return None;
        }
        let n = points.len() as f64;
        let (sum_x, sum_y, sum_xy, sum_xx) = points.iter().fold(
            (0.0, 0.0, 0.0, 0.0),
            |(sum_x, sum_y, sum_xy, sum_xx), (lcn, freq)| {
                let (x, y) = (f64::from(*lcn), *freq as f64);
                (sum_x + x, sum_y + y, sum_xy + x * y, sum_xx + x * x)
            },
        );
        let denominator = n * sum_xx - sum_x * sum_x;
        if denominator.abs() < f64::EPSILON {
            return None;
        }
        let slope = (n * sum_xy - sum_x * sum_y) / denominator;
        if !slope.is_finite() || slope <= 0.0 {
            return None;
        }
        let step_hz =
            (slope / FIT_STEP_GRANULARITY_HZ as f64).round() as i64 * FIT_STEP_GRANULARITY_HZ;
        if step_hz <= 0 || (slope - step_hz as f64).abs() > FIT_TOLERANCE_HZ as f64 {
            return None;
        }
        let mut bases: Vec<i64> = points
            .iter()
            .map(|(lcn, freq)| *freq as i64 - step_hz * i64::from(*lcn))
            .collect();
        bases.sort_unstable();
        let base_hz = bases[bases.len() / 2];
        let plan = Self { base_hz, step_hz };
        points
            .iter()
            .all(|(lcn, freq)| {
                plan.at(*lcn)
                    .is_some_and(|at| at.abs_diff(*freq) <= FIT_TOLERANCE_HZ as u64)
            })
            .then_some(plan)
    }
}

/// A candidate the band is loud on: when the run of loudness started, when it was last seen, and
/// how many snapshots have backed it up.
#[derive(Clone, Copy, Debug)]
struct Sighting {
    seen: Instant,
    count: u32,
    level_db: f32,
}

#[derive(Clone, Copy, Debug)]
struct PendingGrant {
    logical_channel: u16,
    slot: u8,
    destination: u32,
    source: Option<u32>,
    expires: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Match {
    pub logical_channel: u16,
    pub slot: u8,
    pub freq_hz: u64,
    pub confirmed: bool,
}

pub(crate) struct Prospector {
    candidates: Vec<u64>,
    cursor: usize,
    manual: HashMap<u16, u64>,
    learned: HashMap<u16, u64>,
    scores: HashMap<(u16, u64), i32>,
    pending: Vec<PendingGrant>,
    wanted: HashMap<u16, Instant>,
    control: HashSet<u64>,
    payload: HashSet<u64>,
    dwell: HashMap<u64, Instant>,
    busy: HashMap<u64, Sighting>,
    plan: Option<BandPlan>,
    color_code: Option<u8>,
    probes: u8,
    enabled: bool,
    spanning: bool,
}

impl Prospector {
    pub(crate) fn new(discovery: &DmrDiscovery, channel_map: &[DmrChannelEntry]) -> Self {
        let mut prospector = Self {
            candidates: Vec::new(),
            cursor: 0,
            manual: HashMap::new(),
            learned: HashMap::new(),
            scores: HashMap::new(),
            pending: Vec::new(),
            wanted: HashMap::new(),
            control: HashSet::new(),
            payload: HashSet::new(),
            dwell: HashMap::new(),
            busy: HashMap::new(),
            plan: None,
            color_code: None,
            probes: 0,
            enabled: false,
            spanning: false,
        };
        prospector.configure(discovery, channel_map);
        prospector
    }

    pub(crate) fn configure(&mut self, discovery: &DmrDiscovery, channel_map: &[DmrChannelEntry]) {
        self.manual = channel_map
            .iter()
            .map(|entry| (entry.lcn, entry.freq_hz))
            .collect();
        self.probes = discovery.probes();
        self.enabled = discovery.enabled && discovery.valid();
        self.spanning = self.enabled && discovery.ranges.is_empty();
        let mut candidates: Vec<u64> = if self.enabled {
            discovery
                .ranges
                .iter()
                .flat_map(sdrmm_wire::DmrSearchRange::frequencies)
                .collect()
        } else {
            Vec::new()
        };
        candidates.sort_unstable();
        candidates.dedup();
        if !self.spanning {
            self.adopt_candidates(candidates);
        }
        self.refit();
    }

    /// A search the operator turned on without naming a band covers everything the radio can
    /// already hear, stepped off the control channel the site itself transmits on.
    pub(crate) fn cover(&mut self, control_hz: u64, reach: &std::ops::RangeInclusive<u64>) {
        if !self.spanning {
            return;
        }
        let below = control_hz.saturating_sub(*reach.start()) / RASTER_HZ;
        let above = reach.end().saturating_sub(control_hz) / RASTER_HZ;
        let first = control_hz.saturating_sub(below * RASTER_HZ);
        let candidates: Vec<u64> = (0..=below + above)
            .map(|step| first + step * RASTER_HZ)
            .filter(|freq_hz| reach.contains(freq_hz))
            .take(sdrmm_wire::MAX_DMR_SEARCH_CANDIDATES)
            .collect();
        self.adopt_candidates(candidates);
    }

    fn adopt_candidates(&mut self, candidates: Vec<u64>) {
        if candidates != self.candidates {
            self.candidates = candidates;
            self.cursor = 0;
        }
    }

    fn refit(&mut self) {
        let mut points: Vec<(u16, u64)> = self
            .manual
            .iter()
            .chain(
                self.learned
                    .iter()
                    .filter(|(lcn, _)| !self.manual.contains_key(lcn)),
            )
            .map(|(lcn, freq_hz)| (*lcn, *freq_hz))
            .collect();
        points.sort_unstable();
        self.plan = BandPlan::fit(&points);
    }

    fn predicted(&self, logical_channel: u16) -> Option<u64> {
        let freq_hz = self.plan?.at(logical_channel)?;
        let lowest = *self.candidates.first()?;
        let highest = *self.candidates.last()?;
        (lowest..=highest).contains(&freq_hz).then_some(freq_hz)
    }

    pub(crate) fn frequency_of(&self, logical_channel: u16) -> Option<u64> {
        self.manual
            .get(&logical_channel)
            .or_else(|| self.learned.get(&logical_channel))
            .copied()
    }

    pub(crate) fn searching(&self) -> u32 {
        self.wanted.len() as u32
    }

    pub(crate) fn candidates(&self) -> u32 {
        self.candidates.len() as u32
    }

    pub(crate) fn color_code(&self) -> Option<u8> {
        self.color_code
    }

    pub(crate) fn note_site(&mut self, color_code: u8) {
        if self.color_code == Some(color_code) {
            return;
        }
        self.color_code = Some(color_code);
        self.scores.clear();
        self.busy.clear();
    }

    pub(crate) fn adopt(&mut self, channels: &[DmrChannelEntry]) {
        for entry in channels {
            self.learned.entry(entry.lcn).or_insert(entry.freq_hz);
        }
        self.refit();
    }

    /// Reports whether a candidate has just become loud enough, for long enough, to be worth
    /// interrupting the sweep for.
    pub(crate) fn note_carriers(&mut self, heard: &[Heard], now: Instant) -> bool {
        if !self.enabled || self.wanted.is_empty() {
            return false;
        }
        self.busy
            .retain(|_, sighting| now.duration_since(sighting.seen) < CARRIER_MEMORY);
        let mut woke = false;
        for observed in heard {
            let Some(candidate) = self.nearest_candidate(observed.freq_hz) else {
                continue;
            };
            let sighting = self.busy.entry(candidate).or_insert(Sighting {
                seen: now,
                count: 0,
                level_db: observed.level_db,
            });
            sighting.seen = now;
            sighting.count += 1;
            sighting.level_db = sighting.level_db.max(observed.level_db);
            woke |= sighting.count == CARRIER_SIGHTINGS;
        }
        woke
    }

    fn nearest_candidate(&self, freq_hz: u64) -> Option<u64> {
        let at = self
            .candidates
            .partition_point(|candidate| *candidate < freq_hz);
        [at.checked_sub(1), Some(at)]
            .into_iter()
            .flatten()
            .filter_map(|index| self.candidates.get(index).copied())
            .min_by_key(|candidate| candidate.abs_diff(freq_hz))
            .filter(|candidate| candidate.abs_diff(freq_hz) <= CARRIER_SNAP_HZ)
    }

    pub(crate) fn note_grant(
        &mut self,
        logical_channel: u16,
        slot: u8,
        destination: u32,
        source: Option<u32>,
        now: Instant,
    ) -> bool {
        if !self.enabled || self.frequency_of(logical_channel).is_some() {
            return false;
        }
        let fresh = self.wanted.insert(logical_channel, now).is_none();
        self.pending.retain(|grant| grant.expires > now);
        let source = source.filter(|source| *source != 0);
        if let Some(grant) = self.pending.iter_mut().find(|grant| {
            grant.logical_channel == logical_channel
                && grant.slot == slot
                && grant.destination == destination
        }) {
            grant.expires = now + GRANT_WINDOW;
            grant.source = grant.source.or(source);
            return fresh;
        }
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push(PendingGrant {
            logical_channel,
            slot,
            destination,
            source,
            expires: now + GRANT_WINDOW,
        });
        fresh
    }

    pub(crate) fn note_probe(
        &mut self,
        freq_hz: u64,
        frame: &DvFrame,
        now: Instant,
    ) -> Option<Match> {
        if frame.control_channel == Some(false) {
            self.payload.insert(freq_hz);
            self.control.remove(&freq_hz);
        }
        if frame.control_channel == Some(true) {
            self.payload.remove(&freq_hz);
            self.control.insert(freq_hz);
            return None;
        }
        if let Some(found) = self.note_late_entry(freq_hz, frame, now) {
            return Some(found);
        }
        if !identifies_a_call(frame) {
            return None;
        }
        if let Some(site) = self.color_code
            && frame.color_code != Some(u16::from(site))
        {
            return None;
        }
        let destination = frame.destination?;
        self.pending.retain(|grant| grant.expires > now);
        let (logical_channel, slot) = self.sole_candidate(frame, destination)?;
        let identified = self.names_both_radios(logical_channel, frame);
        if frame.kind == DvFrameKind::Data && !identified {
            return None;
        }
        let weight = if identified {
            IDENTIFIED_WEIGHT
        } else {
            ANONYMOUS_WEIGHT
        };
        Some(self.credit(logical_channel, slot, freq_hz, weight, now))
    }

    /// A traffic channel repeats the grant for the call it is carrying so a radio can join late,
    /// which names the logical channel the transmitter is sitting on. Only a frequency the CACH
    /// has already called a payload channel is read this way, so a neighbouring control channel
    /// handing out somebody else's channel is never mistaken for it - and only a channel grant,
    /// because the announcements that move a radio elsewhere name a channel they are not on.
    fn note_late_entry(&mut self, freq_hz: u64, frame: &DvFrame, now: Instant) -> Option<Match> {
        if frame.kind != DvFrameKind::Control
            || frame.late_entry.is_none()
            || !self.payload.contains(&freq_hz)
        {
            return None;
        }
        let logical_channel = frame.channel?;
        if self.frequency_of(logical_channel).is_some() {
            return None;
        }
        let slot = frame.slot.unwrap_or(1);
        Some(self.credit(logical_channel, slot, freq_hz, ANONYMOUS_WEIGHT, now))
    }

    fn credit(
        &mut self,
        logical_channel: u16,
        slot: u8,
        freq_hz: u64,
        weight: i32,
        now: Instant,
    ) -> Match {
        self.contradict(logical_channel, freq_hz);
        self.dwell.insert(freq_hz, now);
        let score = self.scores.entry((logical_channel, freq_hz)).or_insert(0);
        *score = (*score + weight).min(MAX_SCORE);
        let score = *score;
        let confirmed =
            score >= CONFIRM_SCORE && self.runner_up(logical_channel, freq_hz) * 2 < score;
        if confirmed {
            self.learned.insert(logical_channel, freq_hz);
            self.wanted.remove(&logical_channel);
            self.pending
                .retain(|grant| grant.logical_channel != logical_channel);
            self.refit();
        }
        Match {
            logical_channel,
            slot,
            freq_hz,
            confirmed,
        }
    }

    fn sole_candidate(&self, frame: &DvFrame, destination: u32) -> Option<(u16, u8)> {
        let mut found: Option<(u16, u8)> = None;
        for grant in &self.pending {
            if frame.slot.is_some_and(|slot| slot != grant.slot) {
                continue;
            }
            if !answers(grant, frame, destination) {
                continue;
            }
            match found {
                Some((logical_channel, _)) if logical_channel != grant.logical_channel => {
                    return None;
                }
                _ => found = Some((grant.logical_channel, grant.slot)),
            }
        }
        found
    }

    /// Whether the burst names the two radios the grant named, whichever way round. Both halves
    /// of a conversation run on the channel that was handed out, so an answer is as good a
    /// witness as the call that prompted it - and naming both radios is a stricter test than
    /// matching the called party alone.
    fn names_both_radios(&self, logical_channel: u16, frame: &DvFrame) -> bool {
        let (Some(source), Some(destination)) = (frame.source, frame.destination) else {
            return false;
        };
        self.pending.iter().any(|grant| {
            grant.logical_channel == logical_channel
                && ((grant.source == Some(source) && grant.destination == destination)
                    || (grant.source == Some(destination) && grant.destination == source))
        })
    }

    fn contradict(&mut self, logical_channel: u16, freq_hz: u64) {
        let stale: Vec<(u16, u64)> = self
            .scores
            .keys()
            .filter(|(lcn, other)| *lcn == logical_channel && *other != freq_hz)
            .copied()
            .collect();
        for key in stale {
            if let Some(score) = self.scores.get_mut(&key) {
                *score -= CONTRADICTION_WEIGHT;
                if *score <= 0 {
                    self.scores.remove(&key);
                }
            }
        }
    }

    fn runner_up(&self, logical_channel: u16, freq_hz: u64) -> i32 {
        self.scores
            .iter()
            .filter(|((lcn, other), _)| *lcn == logical_channel && *other != freq_hz)
            .map(|(_, score)| *score)
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn schedule(
        &mut self,
        now: Instant,
        open: &[u64],
        usable: impl Fn(u64) -> bool,
    ) -> Vec<u64> {
        self.pending.retain(|grant| grant.expires > now);
        self.wanted
            .retain(|_, since| now.duration_since(*since) < SEARCH_IDLE);
        if !self.enabled || self.wanted.is_empty() || self.candidates.is_empty() {
            self.dwell.clear();
            self.busy.clear();
            return Vec::new();
        }
        let budget = usize::from(self.probes);
        let mut chosen: Vec<u64> = Vec::with_capacity(budget);
        let mut expired: Vec<u64> = Vec::new();
        let mut holding: Vec<u64> = Vec::new();
        for freq_hz in open {
            if !self.probeable(*freq_hz, &usable) {
                continue;
            }
            let since = self.dwell.get(freq_hz).copied().unwrap_or(now);
            if now.duration_since(since) >= DWELL {
                expired.push(*freq_hz);
            } else {
                holding.push(*freq_hz);
            }
        }
        let mut heard: Vec<(f32, u64)> = self
            .busy
            .iter()
            .filter(|(_, sighting)| {
                sighting.count >= CARRIER_SIGHTINGS
                    && now.duration_since(sighting.seen) < CARRIER_MEMORY
            })
            .map(|(freq_hz, sighting)| (sighting.level_db, *freq_hz))
            .collect();
        heard.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, freq_hz) in heard {
            if chosen.len() >= budget {
                break;
            }
            if self.selectable(freq_hz, &chosen, &expired, &usable) {
                chosen.push(freq_hz);
            }
        }
        let mut hot: Vec<(i32, u64)> = self
            .scores
            .iter()
            .filter(|((lcn, _), _)| self.wanted.contains_key(lcn))
            .map(|((_, freq), score)| (*score, *freq))
            .collect();
        hot.sort_unstable_by(|a, b| b.cmp(a));
        for (_, freq_hz) in hot {
            if chosen.len() >= budget {
                break;
            }
            if self.selectable(freq_hz, &chosen, &expired, &usable) {
                chosen.push(freq_hz);
            }
        }
        for freq_hz in holding {
            if chosen.len() >= budget {
                break;
            }
            if self.selectable(freq_hz, &chosen, &expired, &usable) {
                chosen.push(freq_hz);
            }
        }
        let mut foretold: Vec<u16> = self.wanted.keys().copied().collect();
        foretold.sort_unstable();
        for logical_channel in foretold {
            if chosen.len() >= budget {
                break;
            }
            if let Some(freq_hz) = self.predicted(logical_channel)
                && self.selectable(freq_hz, &chosen, &expired, &usable)
            {
                chosen.push(freq_hz);
            }
        }
        for _ in 0..self.candidates.len() {
            if chosen.len() >= budget {
                break;
            }
            let freq_hz = self.candidates[self.cursor % self.candidates.len()];
            self.cursor = (self.cursor + 1) % self.candidates.len();
            if self.selectable(freq_hz, &chosen, &expired, &usable) {
                chosen.push(freq_hz);
            }
        }
        self.dwell.retain(|freq_hz, _| chosen.contains(freq_hz));
        for freq_hz in &chosen {
            self.dwell.entry(*freq_hz).or_insert(now);
        }
        chosen
    }

    fn selectable(
        &self,
        freq_hz: u64,
        chosen: &[u64],
        expired: &[u64],
        usable: &impl Fn(u64) -> bool,
    ) -> bool {
        !chosen.contains(&freq_hz) && !expired.contains(&freq_hz) && self.probeable(freq_hz, usable)
    }

    fn probeable(&self, freq_hz: u64, usable: &impl Fn(u64) -> bool) -> bool {
        !self.control.contains(&freq_hz)
            && !self.learned.values().any(|known| *known == freq_hz)
            && !self.manual.values().any(|known| *known == freq_hz)
            && usable(freq_hz)
    }

    pub(crate) fn channel_map(&self) -> Vec<TrunkChannel> {
        let mut map: Vec<TrunkChannel> = self
            .manual
            .iter()
            .map(|(logical_channel, freq_hz)| TrunkChannel {
                logical_channel: *logical_channel,
                freq_hz: *freq_hz,
                source: TrunkChannelSource::Manual,
                confidence: 100,
            })
            .chain(
                self.learned
                    .iter()
                    .filter(|(logical_channel, _)| !self.manual.contains_key(logical_channel))
                    .map(|(logical_channel, freq_hz)| TrunkChannel {
                        logical_channel: *logical_channel,
                        freq_hz: *freq_hz,
                        source: TrunkChannelSource::Learned,
                        confidence: self.confidence(*logical_channel, *freq_hz),
                    }),
            )
            .collect();
        map.extend(self.wanted.keys().filter_map(|logical_channel| {
            Some(TrunkChannel {
                logical_channel: *logical_channel,
                freq_hz: self.predicted(*logical_channel)?,
                source: TrunkChannelSource::Predicted,
                confidence: 0,
            })
        }));
        map.sort_unstable_by_key(|channel| channel.logical_channel);
        map
    }

    fn confidence(&self, logical_channel: u16, freq_hz: u64) -> u8 {
        let score = self
            .scores
            .get(&(logical_channel, freq_hz))
            .copied()
            .unwrap_or(CONFIRM_SCORE);
        (score.clamp(0, MAX_SCORE) * 100 / MAX_SCORE) as u8
    }
}

/// Whether the burst belongs to the call the grant handed the channel out for: the call itself,
/// addressed to the party that was called, or the answer coming back the other way.
fn answers(grant: &PendingGrant, frame: &DvFrame, destination: u32) -> bool {
    let called = grant.destination == destination
        && match (frame.source, grant.source) {
            (Some(seen), Some(granted)) => seen == granted,
            _ => true,
        };
    let answering = grant.source == Some(destination) && frame.source == Some(grant.destination);
    called || answering
}

/// A data call names its parties in a header that looks much like any other burst, so a system
/// that only ever carries telemetry can still be placed. It is held to the stricter of the two
/// tests below: only a matching radio counts, because a control channel carries short data too.
fn identifies_a_call(frame: &DvFrame) -> bool {
    matches!(
        frame.kind,
        DvFrameKind::Header | DvFrameKind::Voice | DvFrameKind::Terminator | DvFrameKind::Data
    )
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{DmrSearchRange, DvMode, DvTrunkProtocol};

    use super::*;

    const LCN: u16 = 17;
    const TRAFFIC_HZ: u64 = 451_012_500;

    fn discovery() -> DmrDiscovery {
        DmrDiscovery {
            enabled: true,
            ranges: vec![DmrSearchRange {
                start_hz: 451_000_000,
                end_hz: 451_050_000,
                step_hz: 12_500,
            }],
            max_probes: 4,
        }
    }

    fn prospector() -> Prospector {
        Prospector::new(&discovery(), &[])
    }

    fn grant(logical_channel: u16, destination: u32, source: u32) -> DvFrame {
        DvFrame {
            channel: Some(logical_channel),
            late_entry: Some(false),
            slot: Some(1),
            destination: Some(destination),
            source: Some(source),
            trunk_protocol: Some(DvTrunkProtocol::TierThree),
            crc_verified: Some(true),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
        }
    }

    fn voice(destination: u32, source: u32) -> DvFrame {
        DvFrame {
            slot: Some(1),
            destination: Some(destination),
            source: Some(source),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Header)
        }
    }

    fn loud(freq_hz: u64) -> Heard {
        Heard {
            freq_hz,
            level_db: 20.0,
        }
    }

    fn bind(prospector: &mut Prospector, frame: &DvFrame, now: Instant) -> Option<Match> {
        bind_at(prospector, TRAFFIC_HZ, frame, now)
    }

    fn bind_at(
        prospector: &mut Prospector,
        freq_hz: u64,
        frame: &DvFrame,
        now: Instant,
    ) -> Option<Match> {
        let mut last = None;
        for _ in 0..2 {
            last = prospector.note_probe(freq_hz, frame, now);
        }
        last
    }

    #[test]
    fn a_search_without_a_named_band_covers_what_the_radio_can_hear() {
        let mut prospector = Prospector::new(
            &DmrDiscovery {
                enabled: true,
                ranges: Vec::new(),
                max_probes: 4,
            },
            &[],
        );
        let now = Instant::now();
        prospector.cover(460_137_500, &(459_612_500..=461_012_500));
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let chosen = prospector.schedule(now, &[], |_| true);

        assert!(!chosen.is_empty(), "an unnamed search picked nothing");
        assert!(
            prospector.candidates.contains(&460_262_500)
                && prospector.candidates.contains(&460_512_500),
            "the band the radio is tuned to was not stepped off the control channel"
        );
        assert!(
            prospector
                .candidates
                .iter()
                .all(|freq_hz| (459_612_500..=461_012_500).contains(freq_hz)),
            "the search reached past what the radio can hear"
        );
    }

    #[test]
    fn a_named_band_is_never_widened_to_the_whole_radio() {
        let mut prospector = prospector();
        prospector.cover(451_012_500, &(450_000_000..=452_000_000));

        assert_eq!(
            *prospector.candidates.last().expect("candidates"),
            451_050_000
        );
    }

    #[test]
    fn a_carrier_that_holds_for_two_snapshots_asks_for_an_early_look() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        assert!(
            !prospector.note_carriers(&[loud(TRAFFIC_HZ)], now),
            "one loud snapshot was taken for a transmitter"
        );
        assert!(prospector.note_carriers(&[loud(TRAFFIC_HZ)], now));
        assert!(
            !prospector.note_carriers(&[loud(TRAFFIC_HZ)], now),
            "a carrier that was already believed woke the search again"
        );
    }

    #[test]
    fn a_single_loud_snapshot_never_displaces_the_sweep() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.probes = 1;
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        prospector.note_carriers(&[loud(451_037_600)], now);

        assert_ne!(
            prospector.schedule(now, &[], |_| true),
            vec![451_037_500],
            "a one-snapshot noise run stole the only probe"
        );
    }

    #[test]
    fn a_voice_header_matching_a_pending_grant_learns_the_frequency() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let outcome = bind(&mut prospector, &voice(91, 4001), now);

        assert_eq!(
            outcome,
            Some(Match {
                logical_channel: LCN,
                slot: 1,
                freq_hz: TRAFFIC_HZ,
                confirmed: true,
            })
        );
        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
        assert_eq!(prospector.searching(), 0);
    }

    #[test]
    fn one_sighting_is_not_enough_to_learn_a_frequency() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let outcome = prospector.note_probe(TRAFFIC_HZ, &voice(91, 4001), now);

        assert_eq!(outcome.map(|found| found.confirmed), Some(false));
        assert_eq!(prospector.frequency_of(LCN), None);
    }

    #[test]
    fn two_grants_sharing_a_destination_bind_nothing() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        prospector.note_grant(LCN + 1, 1, 91, Some(4002), now);

        let outcome = bind(&mut prospector, &voice(91, 0), now);

        assert!(outcome.is_none(), "an ambiguous sighting bound a channel");
        assert_eq!(prospector.frequency_of(LCN), None);
        assert_eq!(prospector.frequency_of(LCN + 1), None);
    }

    #[test]
    fn a_probe_that_lands_on_a_control_channel_binds_nothing_and_drops_the_frequency() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut announced = grant(LCN, 91, 4001);
        announced.control_channel = Some(true);

        let outcome = prospector.note_probe(TRAFFIC_HZ, &announced, now);

        assert!(outcome.is_none());
        assert!(prospector.frequency_of(LCN).is_none());
        assert!(
            !prospector
                .schedule(now, &[], |_| true)
                .contains(&TRAFFIC_HZ),
            "a known control channel stayed in the search"
        );
    }

    #[test]
    fn a_traffic_channel_that_announces_its_own_call_places_itself() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut cach = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        cach.trunk_protocol = Some(DvTrunkProtocol::TierThree);
        cach.control_channel = Some(false);
        prospector.note_probe(TRAFFIC_HZ, &cach, now);

        let mut last = None;
        for _ in 0..CONFIRM_SCORE {
            last = prospector.note_probe(TRAFFIC_HZ, &grant(LCN, 91, 4001), now);
        }

        assert_eq!(
            last,
            Some(Match {
                logical_channel: LCN,
                slot: 1,
                freq_hz: TRAFFIC_HZ,
                confirmed: true,
            })
        );
        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
    }

    #[test]
    fn an_announcement_naming_another_channel_places_nothing() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut cach = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        cach.trunk_protocol = Some(DvTrunkProtocol::TierThree);
        cach.control_channel = Some(false);
        prospector.note_probe(TRAFFIC_HZ, &cach, now);
        let mut elsewhere = grant(LCN, 91, 4001);
        elsewhere.late_entry = None;

        for _ in 0..CONFIRM_SCORE * 2 {
            prospector.note_probe(TRAFFIC_HZ, &elsewhere, now);
        }

        assert_eq!(prospector.frequency_of(LCN), None);
    }

    #[test]
    fn a_channel_announced_before_the_cach_names_the_channel_kind_is_not_placed() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        for _ in 0..CONFIRM_SCORE * 2 {
            prospector.note_probe(TRAFFIC_HZ, &grant(LCN, 91, 4001), now);
        }

        assert_eq!(prospector.frequency_of(LCN), None);
    }

    #[test]
    fn a_control_frame_never_counts_as_traffic() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut control = voice(91, 4001);
        control.kind = DvFrameKind::Control;

        assert!(prospector.note_probe(TRAFFIC_HZ, &control, now).is_none());
    }

    #[test]
    fn a_site_that_checksums_its_own_way_still_places_a_channel() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut unchecked = voice(91, 4001);
        unchecked.crc_verified = Some(false);

        let found = prospector
            .note_probe(TRAFFIC_HZ, &unchecked, now)
            .expect("an unverified burst was thrown away");

        assert_eq!(found.logical_channel, LCN);
        assert_eq!(found.freq_hz, TRAFFIC_HZ);
    }

    #[test]
    fn a_traffic_channel_that_repeats_a_grant_is_not_taken_for_a_control_channel() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut payload = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        payload.trunk_protocol = Some(DvTrunkProtocol::TierThree);
        payload.control_channel = Some(false);

        prospector.note_probe(TRAFFIC_HZ, &payload, now);
        prospector.note_probe(TRAFFIC_HZ, &grant(LCN, 91, 4001), now);

        assert!(
            prospector
                .schedule(now, &[], |_| true)
                .contains(&TRAFFIC_HZ),
            "the traffic channel was blacklisted as another site's control channel"
        );
    }

    #[test]
    fn a_neighbouring_control_channel_leaves_the_search() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut control = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        control.trunk_protocol = Some(DvTrunkProtocol::TierThree);
        control.control_channel = Some(true);

        prospector.note_probe(TRAFFIC_HZ, &control, now);

        assert!(
            !prospector
                .schedule(now, &[], |_| true)
                .contains(&TRAFFIC_HZ),
            "a control channel stayed in the traffic search"
        );
    }

    #[test]
    fn a_repeated_channel_update_refreshes_one_grant_instead_of_piling_up() {
        let mut prospector = prospector();
        let now = Instant::now();
        for _ in 0..MAX_PENDING * 2 {
            prospector.note_grant(LCN, 1, 91, None, now);
        }
        prospector.note_grant(LCN + 1, 2, 92, Some(4002), now);

        assert_eq!(
            prospector.pending.len(),
            2,
            "repeated updates evicted the grants they were meant to sit beside"
        );
        assert_eq!(prospector.pending[1].logical_channel, LCN + 1);
    }

    #[test]
    fn a_repeated_grant_keeps_the_radio_it_learned_the_first_time() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        prospector.note_grant(LCN, 1, 91, None, now);

        assert_eq!(prospector.pending.len(), 1);
        assert_eq!(prospector.pending[0].source, Some(4001));
    }

    #[test]
    fn a_sighting_after_the_grant_window_binds_nothing() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let late = now + GRANT_WINDOW + Duration::from_secs(1);
        assert!(
            prospector
                .note_probe(TRAFFIC_HZ, &voice(91, 4001), late)
                .is_none()
        );
    }

    #[test]
    fn a_manual_entry_answers_without_any_search() {
        let prospector = Prospector::new(
            &discovery(),
            &[DmrChannelEntry {
                lcn: LCN,
                freq_hz: TRAFFIC_HZ,
            }],
        );

        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
    }

    #[test]
    fn a_grant_for_a_known_channel_starts_no_search() {
        let mut prospector = Prospector::new(
            &discovery(),
            &[DmrChannelEntry {
                lcn: LCN,
                freq_hz: TRAFFIC_HZ,
            }],
        );
        let now = Instant::now();

        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        assert_eq!(prospector.searching(), 0);
        assert!(prospector.schedule(now, &[], |_| true).is_empty());
    }

    #[test]
    fn nothing_is_probed_until_a_grant_asks_for_an_unknown_channel() {
        let mut prospector = prospector();
        let now = Instant::now();
        assert!(prospector.schedule(now, &[], |_| true).is_empty());

        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        assert_eq!(prospector.schedule(now, &[], |_| true).len(), 4);
    }

    #[test]
    fn the_search_stops_once_the_grants_go_quiet() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        assert!(!prospector.schedule(now, &[], |_| true).is_empty());

        let later = now + SEARCH_IDLE + Duration::from_secs(1);

        assert!(prospector.schedule(later, &[], |_| true).is_empty());
        assert_eq!(prospector.searching(), 0);
    }

    #[test]
    fn the_search_walks_the_whole_raster() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut seen: HashSet<u64> = HashSet::new();
        for _ in 0..2 {
            seen.extend(prospector.schedule(now, &[], |_| true));
        }

        assert_eq!(seen.len(), 5, "the raster was not covered");
    }

    #[test]
    fn frequencies_outside_the_sampled_band_are_never_probed() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let chosen = prospector.schedule(now, &[], |freq_hz| freq_hz <= 451_012_500);

        assert_eq!(chosen.len(), 2);
        assert!(chosen.iter().all(|freq_hz| *freq_hz <= 451_012_500));
    }

    fn a_plan_over(entries: &[(u16, u64)]) -> Prospector {
        let map: Vec<DmrChannelEntry> = entries
            .iter()
            .map(|(lcn, freq_hz)| DmrChannelEntry {
                lcn: *lcn,
                freq_hz: *freq_hz,
            })
            .collect();
        Prospector::new(&discovery(), &map)
    }

    #[test]
    fn three_channels_on_a_raster_predict_the_rest_of_the_plan() {
        let prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500), (3, 451_025_000)]);

        assert_eq!(
            prospector.plan,
            Some(BandPlan {
                base_hz: 450_987_500,
                step_hz: 12_500,
            })
        );
        assert_eq!(prospector.predicted(4), Some(451_037_500));
    }

    #[test]
    fn two_channels_are_never_enough_to_call_it_a_plan() {
        let prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500)]);

        assert_eq!(prospector.plan, None);
        assert_eq!(prospector.predicted(3), None);
    }

    #[test]
    fn a_channel_that_breaks_the_raster_throws_the_plan_out() {
        let prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500), (3, 451_040_000)]);

        assert_eq!(prospector.plan, None);
    }

    #[test]
    fn a_plan_never_predicts_outside_the_area_it_was_told_to_search() {
        let prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500), (3, 451_025_000)]);

        assert_eq!(prospector.predicted(4), Some(451_037_500));
        assert_eq!(
            prospector.predicted(400),
            None,
            "the plan reached past the search area"
        );
    }

    #[test]
    fn a_predicted_frequency_is_probed_before_the_rest_of_the_raster() {
        let mut prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500), (3, 451_025_000)]);
        let now = Instant::now();
        prospector.probes = 1;

        prospector.note_grant(4, 1, 91, Some(4001), now);

        assert_eq!(prospector.schedule(now, &[], |_| true), vec![451_037_500]);
    }

    #[test]
    fn a_prediction_is_never_followed_until_traffic_confirms_it() {
        let mut prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500), (3, 451_025_000)]);
        let now = Instant::now();
        prospector.note_grant(4, 1, 91, Some(4001), now);

        assert_eq!(
            prospector.frequency_of(4),
            None,
            "a guess was handed out as a real frequency"
        );
        assert!(
            prospector
                .channel_map()
                .iter()
                .any(|channel| channel.logical_channel == 4
                    && channel.source == TrunkChannelSource::Predicted
                    && channel.confidence == 0)
        );
    }

    #[test]
    fn confirming_a_channel_rebuilds_the_plan_around_it() {
        let mut prospector = a_plan_over(&[(1, 451_000_000), (2, 451_012_500)]);
        let now = Instant::now();
        assert_eq!(prospector.plan, None);

        prospector.note_grant(3, 1, 91, Some(4001), now);
        bind_at(&mut prospector, 451_025_000, &voice(91, 4001), now);

        assert_eq!(prospector.predicted(4), Some(451_037_500));
    }

    #[test]
    fn traffic_from_a_neighbouring_site_never_places_a_channel() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_site(3);
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let mut neighbour = voice(91, 4001);
        neighbour.color_code = Some(7);

        assert!(
            bind(&mut prospector, &neighbour, now).is_none(),
            "a wide-area call on another site was bound to this site's channel"
        );
        assert_eq!(prospector.frequency_of(LCN), None);
    }

    #[test]
    fn traffic_from_this_site_still_places_a_channel() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_site(3);
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        let mut here = voice(91, 4001);
        here.color_code = Some(3);

        assert_eq!(
            bind(&mut prospector, &here, now).map(|found| found.confirmed),
            Some(true)
        );
        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
    }

    #[test]
    fn moving_to_another_site_throws_away_what_was_half_learned_there() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_site(3);
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        let mut here = voice(91, 4001);
        here.color_code = Some(3);
        prospector.note_probe(TRAFFIC_HZ, &here, now);
        assert!(!prospector.scores.is_empty());

        prospector.note_site(7);

        assert!(
            prospector.scores.is_empty(),
            "evidence gathered at one site was carried to the next"
        );
        assert_eq!(prospector.color_code(), Some(7));
    }

    #[test]
    fn a_saved_plan_comes_back_as_something_the_search_found() {
        let mut prospector = prospector();

        prospector.adopt(&[DmrChannelEntry {
            lcn: LCN,
            freq_hz: TRAFFIC_HZ,
        }]);

        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
        let map = prospector.channel_map();
        assert_eq!(map.len(), 1);
        assert_eq!(map[0].logical_channel, LCN);
        assert_eq!(map[0].source, TrunkChannelSource::Learned);
    }

    #[test]
    fn a_saved_plan_never_overwrites_what_the_search_confirmed_itself() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        bind(&mut prospector, &voice(91, 4001), now);

        prospector.adopt(&[DmrChannelEntry {
            lcn: LCN,
            freq_hz: 451_000_000,
        }]);

        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
    }

    fn data_call(destination: u32, source: u32) -> DvFrame {
        DvFrame {
            slot: Some(1),
            destination: Some(destination),
            source: Some(source),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Data)
        }
    }

    #[test]
    fn a_system_that_only_carries_data_can_still_be_placed() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 9_999, Some(9_998), now);

        let found = bind(&mut prospector, &data_call(9_999, 9_998), now);

        assert_eq!(found.map(|found| found.confirmed), Some(true));
        assert_eq!(prospector.frequency_of(LCN), Some(TRAFFIC_HZ));
    }

    #[test]
    fn a_data_call_that_names_no_radio_places_nothing() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 9_999, None, now);

        assert!(
            bind(&mut prospector, &data_call(9_999, 9_998), now).is_none(),
            "short data on a control channel was read as a placed call"
        );
        assert_eq!(prospector.frequency_of(LCN), None);
    }

    #[test]
    fn a_carrier_heard_on_the_air_is_probed_before_the_blind_sweep() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.probes = 1;
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        prospector.note_carriers(&[loud(451_037_600)], now);
        prospector.note_carriers(&[loud(451_037_600)], now);

        assert_eq!(
            prospector.schedule(now, &[], |_| true),
            vec![451_037_500],
            "the search ignored a carrier that was already on the air"
        );
    }

    #[test]
    fn a_carrier_between_two_raster_steps_is_left_alone() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        prospector.note_carriers(&[loud(451_006_250)], now);

        assert!(
            prospector.busy.is_empty(),
            "a carrier nowhere near the raster was snapped onto it"
        );
    }

    #[test]
    fn a_carrier_nobody_is_looking_for_is_not_remembered() {
        let mut prospector = prospector();
        let now = Instant::now();

        prospector.note_carriers(&[loud(451_012_500)], now);

        assert!(prospector.busy.is_empty());
    }

    #[test]
    fn a_carrier_that_went_quiet_stops_being_probed_first() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.probes = 1;
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        prospector.note_carriers(&[loud(451_037_500)], now);

        let later = now + CARRIER_MEMORY + Duration::from_secs(1);

        assert_ne!(prospector.schedule(later, &[], |_| true), vec![451_037_500]);
    }

    #[test]
    fn a_disabled_search_probes_nothing() {
        let mut prospector = Prospector::new(&DmrDiscovery::default(), &[]);
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);

        assert!(prospector.schedule(now, &[], |_| true).is_empty());
        assert_eq!(prospector.searching(), 0);
    }

    #[test]
    fn a_competing_frequency_loses_its_score_when_the_call_shows_up_elsewhere() {
        let mut prospector = prospector();
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        prospector.note_probe(451_000_000, &voice(91, 4001), now);
        prospector.note_probe(451_000_000, &voice(91, 4001), now);

        assert_eq!(prospector.frequency_of(LCN), Some(451_000_000));

        prospector.note_grant(LCN + 1, 1, 92, Some(4002), now);
        prospector.note_probe(451_000_000, &voice(92, 4002), now);

        assert_eq!(
            prospector.scores.get(&(LCN + 1, 451_000_000)),
            Some(&IDENTIFIED_WEIGHT)
        );
    }

    #[test]
    fn the_published_map_names_where_every_frequency_came_from() {
        let mut prospector = Prospector::new(
            &discovery(),
            &[DmrChannelEntry {
                lcn: 1,
                freq_hz: 451_000_000,
            }],
        );
        let now = Instant::now();
        prospector.note_grant(LCN, 1, 91, Some(4001), now);
        bind(&mut prospector, &voice(91, 4001), now);

        let map = prospector.channel_map();

        assert_eq!(map.len(), 2);
        assert_eq!(map[0].logical_channel, 1);
        assert_eq!(map[0].source, TrunkChannelSource::Manual);
        assert_eq!(map[0].confidence, 100);
        assert_eq!(map[1].logical_channel, LCN);
        assert_eq!(map[1].source, TrunkChannelSource::Learned);
        assert_eq!(map[1].freq_hz, TRAFFIC_HZ);
        assert!(map[1].confidence > 0);
    }
}
