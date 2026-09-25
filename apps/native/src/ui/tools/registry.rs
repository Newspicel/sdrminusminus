use sdrmm_wire::tools::{ToolCategory, ToolDescriptor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Antenna,
    Cps,
    NanoVna,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Standard,
    Full,
}

struct Entry {
    id: &'static str,
    panel: Panel,
    size: Size,
    local: Option<fn() -> ToolDescriptor>,
}

const PANELS: [Entry; 3] = [
    Entry {
        id: "antenna",
        panel: Panel::Antenna,
        size: Size::Standard,
        local: None,
    },
    Entry {
        id: "cps",
        panel: Panel::Cps,
        size: Size::Full,
        local: Some(programmer),
    },
    Entry {
        id: "nanovna",
        panel: Panel::NanoVna,
        size: Size::Full,
        local: None,
    },
];

fn programmer() -> ToolDescriptor {
    ToolDescriptor {
        id: "cps".to_owned(),
        name: "Radio programmer".to_owned(),
        summary: "Read, edit and write radio codeplugs, and copy them between radios".to_owned(),
        category: ToolCategory::Instrument,
        needs_hardware: true,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Launchable {
    pub descriptor: ToolDescriptor,
    pub panel: Option<Panel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub category: ToolCategory,
    pub label: &'static str,
    pub tools: Vec<Launchable>,
}

const CATEGORIES: [(ToolCategory, &str); 3] = [
    (ToolCategory::Instrument, "Instruments"),
    (ToolCategory::Calculator, "Calculators"),
    (ToolCategory::Reference, "Reference"),
];

fn entry_of(id: &str) -> Option<&'static Entry> {
    PANELS.iter().find(|entry| entry.id == id)
}

#[must_use]
pub fn size_of(id: Option<&str>) -> Size {
    id.and_then(entry_of)
        .map_or(Size::Standard, |entry| entry.size)
}

#[must_use]
pub fn launchable(descriptors: &[ToolDescriptor]) -> Vec<Launchable> {
    let served = descriptors.iter().map(|descriptor| Launchable {
        descriptor: descriptor.clone(),
        panel: entry_of(&descriptor.id).map(|entry| entry.panel),
    });
    let local = PANELS
        .iter()
        .filter(|entry| {
            !descriptors
                .iter()
                .any(|descriptor| descriptor.id == entry.id)
        })
        .filter_map(|entry| {
            entry.local.map(|make| Launchable {
                descriptor: make(),
                panel: Some(entry.panel),
            })
        });
    served.chain(local).collect()
}

#[must_use]
pub fn grouped(tools: &[Launchable]) -> Vec<Group> {
    CATEGORIES
        .iter()
        .map(|(category, label)| {
            let mut members: Vec<Launchable> = tools
                .iter()
                .filter(|tool| tool.descriptor.category == *category)
                .cloned()
                .collect();
            members.sort_by_key(|tool| tool.descriptor.name.to_lowercase());
            Group {
                category: *category,
                label,
                tools: members,
            }
        })
        .filter(|group| !group.tools.is_empty())
        .collect()
}

#[must_use]
pub fn find(tools: &[Launchable], id: Option<&str>) -> Option<Launchable> {
    let id = id?;
    tools.iter().find(|tool| tool.descriptor.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(id: &str, name: &str, category: ToolCategory) -> ToolDescriptor {
        ToolDescriptor {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: format!("{name} summary"),
            category,
            needs_hardware: false,
        }
    }

    fn client_only() -> usize {
        PANELS.iter().filter(|entry| entry.local.is_some()).count()
    }

    fn served<'a>(tools: &'a [Launchable], id: &str) -> Option<&'a Launchable> {
        tools.iter().find(|tool| tool.descriptor.id == id)
    }

    #[test]
    fn every_advertised_tool_gets_its_panel() {
        let tools = launchable(&[descriptor(
            "antenna",
            "Antenna calculator",
            ToolCategory::Calculator,
        )]);
        assert_eq!(tools.len(), 1 + client_only());
        assert_eq!(
            served(&tools, "antenna").and_then(|tool| tool.panel),
            Some(Panel::Antenna)
        );
    }

    #[test]
    fn the_nanovna_opens_its_instrument_panel() {
        let tools = launchable(&[descriptor("nanovna", "NanoVNA", ToolCategory::Instrument)]);
        assert_eq!(
            served(&tools, "nanovna").and_then(|tool| tool.panel),
            Some(Panel::NanoVna)
        );
    }

    #[test]
    fn a_client_panel_is_offered_when_the_server_advertises_nothing() {
        let tools = launchable(&[]);
        assert_eq!(tools.len(), client_only());
        assert_eq!(
            served(&tools, "cps").and_then(|tool| tool.panel),
            Some(Panel::Cps)
        );
    }

    #[test]
    fn a_client_panel_the_server_also_advertises_is_listed_once() {
        let tools = launchable(&[descriptor(
            "cps",
            "Radio programmer",
            ToolCategory::Instrument,
        )]);
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool.descriptor.id == "cps")
                .count(),
            1
        );
    }

    #[test]
    fn groups_follow_a_fixed_order_and_sort_by_name() {
        let groups = grouped(&launchable(&[
            descriptor("z-calc", "Zed calculator", ToolCategory::Calculator),
            descriptor("a-calc", "Alpha calculator", ToolCategory::Calculator),
            descriptor("vna", "NanoVNA", ToolCategory::Instrument),
        ]));
        let categories: Vec<_> = groups.iter().map(|group| group.category).collect();
        assert_eq!(
            categories,
            [ToolCategory::Instrument, ToolCategory::Calculator]
        );
        let names: Vec<_> = groups[1]
            .tools
            .iter()
            .map(|tool| tool.descriptor.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha calculator", "Zed calculator"]);
        assert!(
            groups[0]
                .tools
                .iter()
                .any(|tool| tool.descriptor.id == "cps")
        );
    }

    #[test]
    fn empty_categories_are_dropped() {
        let groups = grouped(&[Launchable {
            descriptor: descriptor("antenna", "Antenna calculator", ToolCategory::Calculator),
            panel: None,
        }]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "Calculators");
    }

    #[test]
    fn instruments_take_the_whole_window_and_calculators_a_dialog() {
        assert_eq!(size_of(Some("nanovna")), Size::Full);
        assert_eq!(size_of(Some("cps")), Size::Full);
        assert_eq!(size_of(Some("antenna")), Size::Standard);
        assert_eq!(size_of(Some("unknown")), Size::Standard);
        assert_eq!(size_of(None), Size::Standard);
    }

    #[test]
    fn a_tool_is_found_by_id_and_a_missing_one_is_not() {
        let tools = launchable(&[
            descriptor("antenna", "Antenna", ToolCategory::Calculator),
            descriptor("other", "Other", ToolCategory::Calculator),
        ]);
        assert_eq!(
            find(&tools, Some("other")).map(|tool| tool.descriptor.id),
            Some("other".to_owned())
        );
        assert_eq!(find(&tools, Some("gone")), None);
        assert_eq!(find(&tools, None), None);
        assert_eq!(find(&[], Some("antenna")), None);
    }
}
