use std::collections::{BTreeMap, HashMap};

use crate::acars::{AcarsCore, min};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reasm {
    Skipped,
    InProgress,
    Duplicate,
    Complete(String),
    Incomplete,
}

impl Reasm {
    pub fn assstat(&self) -> &'static str {
        match self {
            Reasm::Complete(_) => "complete",
            Reasm::InProgress => "in progress",
            Reasm::Skipped => "skipped",
            Reasm::Duplicate => "duplicate",
            Reasm::Incomplete => "out of sequence",
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct Key {
    tail: String,
    label: String,
    msg_num: String,
}

struct Entry {
    frags: BTreeMap<i32, String>,
    deadline: f64,
}

struct Fragment {
    seq: i32,
    msg_num: String,
    downlink: bool,
}

const UPLINK_SEQ_WRAP: i32 = ('X' as i32) - ('A' as i32);

pub struct Reassembler {
    entries: HashMap<Key, Entry>,
    timeout_secs: f64,
}

fn fragment(core: &AcarsCore) -> Option<Fragment> {
    let block_id = core.block_id?;
    let downlink = block_id.is_ascii_digit();
    let frag = if downlink {
        let split = min::split_downlink(core.msg_num.as_deref()?)?;
        Fragment {
            seq: i32::from(split.seq?),
            msg_num: split.msg_num,
            downlink,
        }
    } else {
        if core.text.is_empty() {
            return None;
        }
        Fragment {
            seq: block_id as i32 - 'A' as i32,
            msg_num: String::new(),
            downlink,
        }
    };
    (frag.seq >= 0).then_some(frag)
}

fn is_contiguous(seqs: &[i32], downlink: bool) -> bool {
    if downlink {
        seqs.first() == Some(&0) && seqs.windows(2).all(|w| w[1] == w[0] + 1)
    } else {
        seqs.windows(2)
            .all(|w| w[1] == w[0] + 1 || (w[0] == UPLINK_SEQ_WRAP - 1 && w[1] == 0))
    }
}

impl Reassembler {
    pub fn new(timeout_secs: f64) -> Self {
        Self {
            entries: HashMap::new(),
            timeout_secs,
        }
    }

    pub fn push(&mut self, core: &AcarsCore, now_secs: f64) -> Reasm {
        self.entries.retain(|_, e| e.deadline > now_secs);

        let Some(frag) = fragment(core) else {
            return Reasm::Skipped;
        };
        let final_block = !core.more_to_come;
        let key = Key {
            tail: core.tail.clone().unwrap_or_default(),
            label: core.label.clone(),
            msg_num: frag.msg_num,
        };
        if final_block && !self.entries.contains_key(&key) && (frag.seq == 0 || !frag.downlink) {
            return Reasm::Skipped;
        }

        let entry = self.entries.entry(key.clone()).or_insert_with(|| Entry {
            frags: BTreeMap::new(),
            deadline: now_secs + self.timeout_secs,
        });
        if entry.frags.contains_key(&frag.seq) {
            return Reasm::Duplicate;
        }
        entry.frags.insert(frag.seq, core.text.clone());

        if !final_block {
            return Reasm::InProgress;
        }
        let Some(entry) = self.entries.remove(&key) else {
            return Reasm::Incomplete;
        };
        let seqs: Vec<i32> = entry.frags.keys().copied().collect();
        if !is_contiguous(&seqs, frag.downlink) {
            return Reasm::Incomplete;
        }
        Reasm::Complete(entry.frags.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(
        tail: &str,
        label: &str,
        block_id: char,
        msg_num: Option<&str>,
        text: &str,
        more: bool,
    ) -> AcarsCore {
        AcarsCore {
            mode: '2',
            tail: Some(tail.into()),
            label: label.into(),
            sublabel: None,
            mfi: None,
            block_id: Some(block_id),
            ack: None,
            flight: Some("UA0001".into()),
            msg_num: msg_num.map(|s| s.into()),
            text: text.into(),
            more_to_come: more,
            reassembled: false,
            assstat: None,
            app: None,
            vdl2_link: None,
        }
    }

    #[test]
    fn single_block_is_skipped() {
        let mut r = Reassembler::new(120.0);
        let c = core("N12345", "H1", '2', Some("M01A"), "HELLO", false);
        assert_eq!(r.push(&c, 0.0), Reasm::Skipped);
    }

    #[test]
    fn downlink_two_blocks_reassemble() {
        let mut r = Reassembler::new(120.0);
        let a = core("N12345", "H1", '2', Some("M01A"), "FIRST-", true);
        let b = core("N12345", "H1", '3', Some("M01B"), "SECOND", false);
        assert_eq!(r.push(&a, 0.0), Reasm::InProgress);
        assert_eq!(r.push(&b, 5.0), Reasm::Complete("FIRST-SECOND".into()));
    }

    #[test]
    fn uplink_blocks_key_on_block_id() {
        let mut r = Reassembler::new(120.0);
        let a = core("N12345", "H1", 'A', None, "/O2.OHMAabcd", true);
        let b = core("N12345", "H1", 'B', None, "efgh", false);
        assert_eq!(r.push(&a, 0.0), Reasm::InProgress);
        assert_eq!(r.push(&b, 1.0), Reasm::Complete("/O2.OHMAabcdefgh".into()));
    }

    #[test]
    fn duplicate_fragment_detected() {
        let mut r = Reassembler::new(120.0);
        let a = core("N12345", "H1", '2', Some("M01A"), "X", true);
        assert_eq!(r.push(&a, 0.0), Reasm::InProgress);
        assert_eq!(r.push(&a, 1.0), Reasm::Duplicate);
    }

    #[test]
    fn timeout_drops_stale_fragments() {
        let mut r = Reassembler::new(120.0);
        let a = core("N12345", "H1", '2', Some("M01A"), "FIRST-", true);
        let b = core("N12345", "H1", '3', Some("M01B"), "SECOND", false);
        assert_eq!(r.push(&a, 0.0), Reasm::InProgress);
        assert_eq!(r.push(&b, 200.0), Reasm::Incomplete);
    }

    #[test]
    fn assstat_names_match_libacars() {
        let mut r = Reassembler::new(120.0);

        let single = core("N12345", "H1", '2', Some("M01A"), "HELLO", false);
        assert_eq!(r.push(&single, 0.0).assstat(), "skipped");

        let a = core("N99999", "H1", '2', Some("M07A"), "FIRST-", true);
        assert_eq!(r.push(&a, 0.0).assstat(), "in progress");

        assert_eq!(r.push(&a, 1.0).assstat(), "duplicate");

        let b = core("N99999", "H1", '3', Some("M07B"), "SECOND", false);
        assert_eq!(r.push(&b, 2.0).assstat(), "complete");

        let mut r2 = Reassembler::new(120.0);
        let f0 = core("N55555", "H1", '2', Some("M09A"), "A", true);
        let f2 = core("N55555", "H1", '4', Some("M09C"), "C", false);
        assert_eq!(r2.push(&f0, 0.0).assstat(), "in progress");
        assert_eq!(r2.push(&f2, 1.0).assstat(), "out of sequence");
    }

    #[test]
    fn interleaved_aircraft_do_not_mix() {
        let mut r = Reassembler::new(120.0);
        let a1 = core("N11111", "H1", '2', Some("M01A"), "AAA", true);
        let b1 = core("N22222", "H1", '2', Some("M55A"), "BBB", true);
        let a2 = core("N11111", "H1", '3', Some("M01B"), "aaa", false);
        let b2 = core("N22222", "H1", '3', Some("M55B"), "bbb", false);
        r.push(&a1, 0.0);
        r.push(&b1, 0.0);
        assert_eq!(r.push(&a2, 1.0), Reasm::Complete("AAAaaa".into()));
        assert_eq!(r.push(&b2, 1.0), Reasm::Complete("BBBbbb".into()));
    }
}
