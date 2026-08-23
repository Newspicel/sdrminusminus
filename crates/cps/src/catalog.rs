use std::collections::HashMap;

use sdrmm_wire::cps::Codeplug;

#[derive(Clone, Debug, Default)]
pub struct NameList {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

impl NameList {
    #[must_use]
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let names: Vec<String> = names.into_iter().map(str::to_owned).collect();
        let index = names
            .iter()
            .enumerate()
            .filter_map(|(position, name)| {
                u32::try_from(position).ok().map(|at| (name.clone(), at))
            })
            .collect();
        Self { names, index }
    }

    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }

    #[must_use]
    pub fn name_at(&self, index: u32) -> Option<&str> {
        usize::try_from(index)
            .ok()
            .and_then(|position| self.names.get(position))
            .map(String::as_str)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub contacts: NameList,
    pub group_lists: NameList,
    pub scan_lists: NameList,
    pub radio_ids: NameList,
    pub channels: NameList,
    pub zones: NameList,
}

impl Catalog {
    #[must_use]
    pub fn of(codeplug: &Codeplug) -> Self {
        Self {
            contacts: NameList::new(codeplug.contacts.iter().map(|item| item.name.as_str())),
            group_lists: NameList::new(codeplug.group_lists.iter().map(|item| item.name.as_str())),
            scan_lists: NameList::new(codeplug.scan_lists.iter().map(|item| item.name.as_str())),
            radio_ids: NameList::new(codeplug.radio_ids.iter().map(|item| item.name.as_str())),
            channels: NameList::new(codeplug.channels.iter().map(|item| item.name.as_str())),
            zones: NameList::new(codeplug.zones.iter().map(|item| item.name.as_str())),
        }
    }
}

#[derive(Debug, Default)]
pub struct UniqueNames {
    seen: HashMap<String, u32>,
}

impl UniqueNames {
    pub fn claim(&mut self, wanted: &str, fallback: &str) -> String {
        let base = if wanted.is_empty() { fallback } else { wanted };
        let count = self.seen.entry(base.to_owned()).or_insert(0);
        *count += 1;
        if *count == 1 {
            base.to_owned()
        } else {
            let candidate = format!("{base} ({count})");
            self.seen.entry(candidate.clone()).or_insert(1);
            candidate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_list_maps_both_ways() {
        let list = NameList::new(["Local", "Repeater"]);
        assert_eq!(list.index_of("Repeater"), Some(1));
        assert_eq!(list.name_at(0), Some("Local"));
        assert_eq!(list.name_at(9), None);
        assert_eq!(list.index_of("missing"), None);
    }

    #[test]
    fn duplicate_names_are_made_unique_so_references_stay_resolvable() {
        let mut names = UniqueNames::default();
        assert_eq!(names.claim("Local", "Channel"), "Local");
        assert_eq!(names.claim("Local", "Channel"), "Local (2)");
        assert_eq!(names.claim("", "Channel"), "Channel");
        assert_eq!(names.claim("", "Channel"), "Channel (2)");
    }
}
