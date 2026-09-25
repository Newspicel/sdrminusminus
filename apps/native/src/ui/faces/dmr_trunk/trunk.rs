use std::collections::{BTreeMap, HashMap};

use sdrmm_wire::{
    decode::DvTrunkProtocol,
    patch::{
        DmrChannelEntry, DmrSearchRange, DmrTrunkProtocol, MAX_DMR_CHANNEL_MAP,
        MAX_DMR_LOGICAL_CHANNEL, MAX_DMR_SEARCH_CANDIDATES, MAX_DMR_SEARCH_RANGES,
        MIN_DMR_SEARCH_STEP_HZ,
    },
    state::{TrunkChannel, TrunkChannelSource, TrunkSystemStatus},
};

use crate::ui::kit_channel::format_hz;

pub const DMR_TRUNK_PROTOCOLS: [(DmrTrunkProtocol, &str); 4] = [
    (DmrTrunkProtocol::Auto, "Auto-detect"),
    (DmrTrunkProtocol::CapacityPlus, "Capacity Plus"),
    (DmrTrunkProtocol::HyteraXpt, "Hytera XPT"),
    (DmrTrunkProtocol::TierThree, "Tier III / Capacity Max"),
];

fn settled(
    protocol: DmrTrunkProtocol,
    detected: Option<DvTrunkProtocol>,
) -> Option<DvTrunkProtocol> {
    match protocol {
        DmrTrunkProtocol::Auto => detected,
        DmrTrunkProtocol::CapacityPlus => Some(DvTrunkProtocol::CapacityPlus),
        DmrTrunkProtocol::HyteraXpt => Some(DvTrunkProtocol::HyteraXpt),
        DmrTrunkProtocol::TierThree => Some(DvTrunkProtocol::TierThree),
    }
}

#[must_use]
pub fn trunk_protocol_label(
    protocol: DmrTrunkProtocol,
    detected: Option<DvTrunkProtocol>,
) -> &'static str {
    match settled(protocol, detected) {
        Some(DvTrunkProtocol::CapacityPlus) => "Capacity Plus",
        Some(DvTrunkProtocol::HyteraXpt) => "Hytera XPT",
        Some(DvTrunkProtocol::TierThree) => "Tier III",
        None => "Listening for signalling",
    }
}

#[must_use]
pub fn follows_tier_three(protocol: DmrTrunkProtocol, detected: Option<DvTrunkProtocol>) -> bool {
    protocol == DmrTrunkProtocol::TierThree
        || (protocol == DmrTrunkProtocol::Auto && detected == Some(DvTrunkProtocol::TierThree))
}

#[must_use]
pub fn plan_label(protocol: DmrTrunkProtocol, detected: Option<DvTrunkProtocol>) -> &'static str {
    if follows_tier_three(protocol, detected) {
        "Channel plan"
    } else {
        "Repeater outputs"
    }
}

fn megahertz(hz: u64) -> String {
    let text = format!("{:.4}", hz as f64 / 1e6);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn parse_line(line: &str) -> Option<DmrSearchRange> {
    let (span, step) = line.split_once('/')?;
    let (start, end) = span.split_once(['-', '–'])?;
    let number = |text: &str| {
        let text = text.trim();
        (!text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == '.'))
            .then(|| text.parse::<f64>().ok())
            .flatten()
    };
    Some(DmrSearchRange {
        start_hz: (number(start)? * 1e6).round() as u64,
        end_hz: (number(end)? * 1e6).round() as u64,
        step_hz: (number(step)? * 1e3).round() as u64,
    })
}

#[must_use]
pub fn parse_search_ranges(text: &str) -> Vec<DmrSearchRange> {
    let mut ranges = Vec::new();
    for line in text.split(['\n', ';', ',']) {
        if let Some(range) = parse_line(line).filter(search_range_valid) {
            ranges.push(range);
        }
        if ranges.len() >= MAX_DMR_SEARCH_RANGES {
            break;
        }
    }
    ranges
}

#[must_use]
pub fn search_range_valid(range: &DmrSearchRange) -> bool {
    range.start_hz > 0
        && range.end_hz >= range.start_hz
        && range.step_hz >= MIN_DMR_SEARCH_STEP_HZ
        && search_candidates(std::slice::from_ref(range)) <= MAX_DMR_SEARCH_CANDIDATES
}

#[must_use]
pub fn search_candidates(ranges: &[DmrSearchRange]) -> usize {
    ranges.iter().map(DmrSearchRange::candidates).sum()
}

#[must_use]
pub fn format_search_ranges(ranges: &[DmrSearchRange]) -> String {
    ranges
        .iter()
        .map(|range| {
            format!(
                "{}-{} / {}",
                megahertz(range.start_hz),
                megahertz(range.end_hz),
                crate::ui::kit_channel::format_number(range.step_hz as f64 / 1e3, None)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[must_use]
pub fn channel_entry(lcn: Option<f64>, mhz: Option<f64>) -> Option<DmrChannelEntry> {
    let (lcn, mhz) = (lcn?, mhz?);
    if lcn.fract() != 0.0 || lcn < 0.0 || lcn > f64::from(MAX_DMR_LOGICAL_CHANNEL) {
        return None;
    }
    let freq_hz = (mhz * 1e6).round();
    if !freq_hz.is_finite() || freq_hz <= 0.0 {
        return None;
    }
    Some(DmrChannelEntry {
        lcn: lcn as u16,
        freq_hz: freq_hz as u64,
    })
}

#[must_use]
pub fn with_channel(entries: &[DmrChannelEntry], entry: DmrChannelEntry) -> Vec<DmrChannelEntry> {
    let mut kept = without_channel(entries, entry.lcn);
    if kept.len() >= MAX_DMR_CHANNEL_MAP {
        return kept;
    }
    kept.push(entry);
    kept.sort_by_key(|entry| entry.lcn);
    kept
}

#[must_use]
pub fn without_channel(entries: &[DmrChannelEntry], lcn: u16) -> Vec<DmrChannelEntry> {
    entries
        .iter()
        .filter(|entry| entry.lcn != lcn)
        .copied()
        .collect()
}

#[must_use]
pub fn parse_control_hz(text: &str) -> Option<u64> {
    let value = text.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then(|| (value * 1e6).round() as u64)
}

#[must_use]
pub fn control_channel_stalled(
    on_iq: bool,
    control_hz: Option<u64>,
    carriers: Option<u32>,
) -> bool {
    on_iq && control_hz.is_some() && carriers == Some(0)
}

#[must_use]
pub fn awaiting_control_channel(on_iq: bool, control_hz: Option<u64>) -> bool {
    on_iq && control_hz.is_none()
}

#[must_use]
pub const fn source_label(source: TrunkChannelSource) -> &'static str {
    match source {
        TrunkChannelSource::Announced => "announced",
        TrunkChannelSource::Manual => "entered",
        TrunkChannelSource::Learned => "found",
        TrunkChannelSource::Predicted => "guessed",
    }
}

#[must_use]
pub const fn source_hint(source: TrunkChannelSource) -> &'static str {
    match source {
        TrunkChannelSource::Announced => "The system broadcast this frequency itself.",
        TrunkChannelSource::Manual => "You entered this frequency.",
        TrunkChannelSource::Learned => {
            "A call answered a grant here, so the frequency is confirmed."
        }
        TrunkChannelSource::Predicted => {
            "Worked out from the channel spacing. Never followed until a call confirms it."
        }
    }
}

#[must_use]
pub const fn usable(source: TrunkChannelSource) -> bool {
    !matches!(source, TrunkChannelSource::Predicted)
}

#[must_use]
pub fn channel_plan_rows(map: &[TrunkChannel], entries: &[DmrChannelEntry]) -> Vec<TrunkChannel> {
    let mut known: BTreeMap<u16, TrunkChannel> = map
        .iter()
        .map(|channel| (channel.logical_channel, *channel))
        .collect();
    for entry in entries {
        known.entry(entry.lcn).or_insert(TrunkChannel {
            logical_channel: entry.lcn,
            freq_hz: entry.freq_hz,
            source: TrunkChannelSource::Manual,
            confidence: 100,
        });
    }
    known.into_values().collect()
}

#[must_use]
pub fn plan_summary(rows: &[TrunkChannel]) -> String {
    if rows.is_empty() {
        return String::from("No logical channels placed yet.");
    }
    let parts: Vec<String> = [
        TrunkChannelSource::Announced,
        TrunkChannelSource::Manual,
        TrunkChannelSource::Learned,
        TrunkChannelSource::Predicted,
    ]
    .into_iter()
    .filter_map(|source| {
        let count = rows.iter().filter(|row| row.source == source).count();
        (count > 0).then(|| format!("{count} {}", source_label(source)))
    })
    .collect();
    let plural = if rows.len() == 1 { "" } else { "s" };
    format!(
        "{} logical channel{plural}: {}.",
        rows.len(),
        parts.join(", ")
    )
}

#[must_use]
pub fn adoptable(map: &[TrunkChannel], entries: &[DmrChannelEntry]) -> Vec<DmrChannelEntry> {
    map.iter()
        .filter(|channel| {
            channel.source == TrunkChannelSource::Learned
                && !entries
                    .iter()
                    .any(|entry| entry.lcn == channel.logical_channel)
        })
        .map(|channel| DmrChannelEntry {
            lcn: channel.logical_channel,
            freq_hz: channel.freq_hz,
        })
        .collect()
}

#[must_use]
pub fn control_channel_label(freq_hz: u64) -> String {
    format!("also control {}", format_hz(freq_hz as f64))
}

#[must_use]
pub fn search_summary(
    ranges: &[DmrSearchRange],
    candidates: u32,
    searching: u32,
    probes: usize,
) -> String {
    let named = search_candidates(ranges);
    let covering = if named > 0 {
        named
    } else {
        candidates as usize
    };
    let place = if named > 0 {
        "the band you named"
    } else {
        "everything the radio can hear"
    };
    if covering == 0 {
        return String::from(
            "Covering everything the radio can hear, once the site identifies itself.",
        );
    }
    if searching == 0 {
        return format!("Covering {place}: {covering} frequencies.");
    }
    let channels = if searching == 1 { "" } else { "s" };
    let receivers = if probes == 1 { "" } else { "s" };
    format!(
        "Hunting {searching} logical channel{channels} across {covering} frequencies with {probes} receiver{receivers}."
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrunkChannelRole {
    Control,
    Call,
    Search,
}

#[must_use]
pub fn trunk_channel_roles(
    trunks: &[TrunkSystemStatus],
    device_set: u32,
) -> HashMap<u32, (String, TrunkChannelRole)> {
    let mut owners = HashMap::new();
    for system in trunks {
        if let Some(control) = system
            .control
            .filter(|control| control.device_set == device_set)
        {
            owners.insert(
                control.channel,
                (system.node.clone(), TrunkChannelRole::Control),
            );
        }
        for follower in system
            .followers
            .iter()
            .filter(|follower| follower.device_set == device_set)
        {
            owners.insert(
                follower.channel,
                (system.node.clone(), TrunkChannelRole::Call),
            );
        }
        for probe in system
            .probes
            .iter()
            .filter(|probe| probe.device_set == device_set)
        {
            owners.insert(
                probe.channel,
                (system.node.clone(), TrunkChannelRole::Search),
            );
        }
    }
    owners
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::state::{TrunkControl, TrunkFollower, TrunkProbe};

    use super::*;

    #[test]
    fn hytera_is_offered_beside_the_motorola_and_tier_three_systems() {
        let labels: Vec<&str> = DMR_TRUNK_PROTOCOLS
            .iter()
            .map(|(_, label)| *label)
            .collect();
        assert_eq!(
            labels,
            vec![
                "Auto-detect",
                "Capacity Plus",
                "Hytera XPT",
                "Tier III / Capacity Max"
            ]
        );
    }

    #[test]
    fn the_protocol_named_is_the_one_settled_on() {
        assert_eq!(
            trunk_protocol_label(DmrTrunkProtocol::HyteraXpt, None),
            "Hytera XPT"
        );
        assert_eq!(
            trunk_protocol_label(DmrTrunkProtocol::Auto, Some(DvTrunkProtocol::CapacityPlus)),
            "Capacity Plus"
        );
        assert_eq!(
            trunk_protocol_label(DmrTrunkProtocol::Auto, Some(DvTrunkProtocol::TierThree)),
            "Tier III"
        );
        assert_eq!(
            trunk_protocol_label(DmrTrunkProtocol::Auto, None),
            "Listening for signalling"
        );
    }

    #[test]
    fn the_plan_is_called_what_it_is_for_each_system() {
        assert_eq!(
            plan_label(DmrTrunkProtocol::TierThree, None),
            "Channel plan"
        );
        assert_eq!(
            plan_label(DmrTrunkProtocol::Auto, Some(DvTrunkProtocol::TierThree)),
            "Channel plan"
        );
        assert_eq!(
            plan_label(DmrTrunkProtocol::CapacityPlus, None),
            "Repeater outputs"
        );
        assert!(!follows_tier_three(
            DmrTrunkProtocol::Auto,
            Some(DvTrunkProtocol::CapacityPlus)
        ));
        assert!(!follows_tier_three(DmrTrunkProtocol::CapacityPlus, None));
    }

    #[test]
    fn a_plan_entry_needs_both_halves_and_sane_values() {
        assert_eq!(
            channel_entry(Some(17.0), Some(451.0125)),
            Some(DmrChannelEntry {
                lcn: 17,
                freq_hz: 451_012_500
            })
        );
        assert_eq!(channel_entry(Some(17.0), None), None);
        assert_eq!(channel_entry(None, Some(451.0125)), None);
        assert_eq!(channel_entry(Some(-1.0), Some(451.0125)), None);
        assert_eq!(channel_entry(Some(99_999.0), Some(451.0125)), None);
        assert_eq!(channel_entry(Some(17.5), Some(451.0125)), None);
        assert_eq!(channel_entry(Some(17.0), Some(0.0)), None);
    }

    #[test]
    fn the_plan_stays_ordered_and_replaces_a_channel_rather_than_doubling_it() {
        let entry = |lcn, freq_hz| DmrChannelEntry { lcn, freq_hz };
        assert_eq!(
            with_channel(&[entry(18, 451_025_000)], entry(17, 451_012_500)),
            vec![entry(17, 451_012_500), entry(18, 451_025_000)]
        );
        assert_eq!(
            with_channel(&[entry(17, 451_012_500)], entry(17, 451_050_000)),
            vec![entry(17, 451_050_000)]
        );
        assert_eq!(
            without_channel(&[entry(17, 1), entry(18, 2)], 17),
            vec![entry(18, 2)]
        );
    }

    #[test]
    fn a_search_range_reads_mhz_over_a_khz_step() {
        assert_eq!(
            parse_search_ranges("451.0-451.5 / 12.5"),
            vec![DmrSearchRange {
                start_hz: 451_000_000,
                end_hz: 451_500_000,
                step_hz: 12_500
            }]
        );
        assert!(parse_search_ranges("450-460 / 1.25").is_empty());
        assert!(parse_search_ranges("451.0-451.1 / 0.5").is_empty());
        assert!(parse_search_ranges("451.5-451.0 / 12.5").is_empty());
        assert_eq!(
            search_candidates(&parse_search_ranges("451.0-451.05 / 12.5")),
            5
        );
        let text = "451-451.5 / 12.5";
        assert_eq!(format_search_ranges(&parse_search_ranges(text)), text);
    }

    #[test]
    fn the_search_says_what_it_is_doing() {
        let named = parse_search_ranges("451.0-451.05 / 12.5");
        assert!(
            search_summary(&named, 0, 0, 0).contains("Covering the band you named: 5 frequencies")
        );
        assert!(
            search_summary(&named, 0, 1, 4)
                .contains("Hunting 1 logical channel across 5 frequencies with 4 receivers")
        );
        assert_eq!(
            search_summary(&[], 192, 0, 0),
            "Covering everything the radio can hear: 192 frequencies."
        );
        assert!(search_summary(&[], 0, 0, 0).contains("once the site identifies itself"));
        assert!(
            search_summary(&[], 192, 2, 4)
                .contains("Hunting 2 logical channels across 192 frequencies with 4 receivers")
        );
    }

    #[test]
    fn another_control_channel_is_named_to_fall_back_on() {
        assert_eq!(
            control_channel_label(460_275_000),
            "also control 460.275 MHz"
        );
    }

    fn channel(logical_channel: u16, freq_hz: u64, source: TrunkChannelSource) -> TrunkChannel {
        TrunkChannel {
            logical_channel,
            freq_hz,
            source,
            confidence: 100,
        }
    }

    #[test]
    fn only_what_the_search_worked_out_is_offered_to_keep() {
        let map = [
            channel(17, 451_012_500, TrunkChannelSource::Learned),
            channel(18, 451_025_000, TrunkChannelSource::Announced),
            channel(19, 451_037_500, TrunkChannelSource::Manual),
        ];
        assert_eq!(
            adoptable(&map, &[]),
            vec![DmrChannelEntry {
                lcn: 17,
                freq_hz: 451_012_500
            }]
        );
        assert!(
            adoptable(
                &map[..1],
                &[DmrChannelEntry {
                    lcn: 17,
                    freq_hz: 451_012_500
                }]
            )
            .is_empty()
        );
        assert!(adoptable(&[channel(20, 1, TrunkChannelSource::Predicted)], &[]).is_empty());
    }

    #[test]
    fn the_plan_table_merges_what_was_typed_under_what_the_server_knows() {
        let entered = [DmrChannelEntry {
            lcn: 17,
            freq_hz: 451_012_500,
        }];
        assert_eq!(
            channel_plan_rows(&[], &entered),
            vec![channel(17, 451_012_500, TrunkChannelSource::Manual)]
        );
        assert_eq!(
            channel_plan_rows(
                &[channel(17, 451_025_000, TrunkChannelSource::Announced)],
                &entered
            ),
            vec![channel(17, 451_025_000, TrunkChannelSource::Announced)]
        );
        let rows = channel_plan_rows(
            &[
                channel(30, 451_050_000, TrunkChannelSource::Learned),
                channel(2, 451_000_000, TrunkChannelSource::Learned),
            ],
            &entered,
        );
        let order: Vec<u16> = rows.iter().map(|row| row.logical_channel).collect();
        assert_eq!(order, vec![2, 17, 30]);
    }

    #[test]
    fn the_summary_counts_where_each_frequency_came_from() {
        let rows = [
            channel(1, 451_000_000, TrunkChannelSource::Announced),
            channel(2, 451_012_500, TrunkChannelSource::Manual),
            channel(3, 451_025_000, TrunkChannelSource::Learned),
            channel(4, 451_037_500, TrunkChannelSource::Predicted),
        ];
        assert_eq!(
            plan_summary(&rows),
            "4 logical channels: 1 announced, 1 entered, 1 found, 1 guessed."
        );
        assert!(plan_summary(&[]).contains("No logical channels"));
        assert!(!usable(TrunkChannelSource::Predicted));
        assert!(usable(TrunkChannelSource::Learned));
        assert!(usable(TrunkChannelSource::Manual));
        assert!(usable(TrunkChannelSource::Announced));
    }

    #[test]
    fn the_control_field_reads_mhz_and_clears_on_nonsense() {
        assert_eq!(parse_control_hz("451.0125"), Some(451_012_500));
        assert_eq!(parse_control_hz(" 451 "), Some(451_000_000));
        assert_eq!(parse_control_hz(""), None);
        assert_eq!(parse_control_hz("abc"), None);
        assert_eq!(parse_control_hz("-451"), None);
    }

    #[test]
    fn a_missing_or_stalled_control_channel_is_said_out_loud() {
        assert!(awaiting_control_channel(true, None));
        assert!(!awaiting_control_channel(true, Some(451_012_500)));
        assert!(!awaiting_control_channel(false, None));
        assert!(control_channel_stalled(true, Some(451_012_500), Some(0)));
        assert!(!control_channel_stalled(true, Some(451_012_500), Some(1)));
        assert!(!control_channel_stalled(true, None, Some(0)));
        assert!(!control_channel_stalled(false, Some(451_012_500), Some(0)));
        assert!(!control_channel_stalled(true, Some(451_012_500), None));
    }

    fn system(
        control: Option<TrunkControl>,
        followers: Vec<TrunkFollower>,
        probes: Vec<TrunkProbe>,
    ) -> TrunkSystemStatus {
        let mut system: TrunkSystemStatus = serde_json::from_value(serde_json::json!({
            "node": "trunk", "carriers": 1, "followers": [], "problems": []
        }))
        .expect("a trunk system");
        system.control = control;
        system.followers = followers;
        system.probes = probes;
        system
    }

    #[test]
    fn a_trunk_system_owns_its_control_call_and_search_receivers_on_one_radio() {
        let roles = trunk_channel_roles(
            &[system(
                Some(TrunkControl {
                    device_set: 1,
                    channel: 9,
                    freq_hz: 460_137_500,
                }),
                vec![TrunkFollower {
                    device_set: 1,
                    channel: 10,
                    logical_channel: Some(22),
                    slot: 2,
                    freq_hz: 460_262_500,
                }],
                vec![TrunkProbe {
                    device_set: 1,
                    channel: 11,
                    freq_hz: 460_512_500,
                }],
            )],
            1,
        );
        assert_eq!(
            roles.get(&9),
            Some(&(String::from("trunk"), TrunkChannelRole::Control))
        );
        assert_eq!(
            roles.get(&10),
            Some(&(String::from("trunk"), TrunkChannelRole::Call))
        );
        assert_eq!(
            roles.get(&11),
            Some(&(String::from("trunk"), TrunkChannelRole::Search))
        );
        let other = trunk_channel_roles(
            &[system(
                Some(TrunkControl {
                    device_set: 1,
                    channel: 9,
                    freq_hz: 460_137_500,
                }),
                vec![TrunkFollower {
                    device_set: 2,
                    channel: 3,
                    logical_channel: Some(42),
                    slot: 1,
                    freq_hz: 460_513_000,
                }],
                Vec::new(),
            )],
            2,
        );
        assert_eq!(other.len(), 1);
        assert_eq!(
            other.get(&3),
            Some(&(String::from("trunk"), TrunkChannelRole::Call))
        );
        assert!(trunk_channel_roles(&[system(None, Vec::new(), Vec::new())], 1).is_empty());
    }
}
