//! Workspaces — the persisted UI shell (PLAN §10, §16 M6). A workspace holds tabs, a tab holds
//! a panel layout, and exactly one workspace is active server-side, so every client that opens
//! the station sees the same setup.
//!
//! The layout is *our* tree, not the dock library's serialization: templates author layouts in
//! Rust (PLAN §16 M6), the server is the source of truth for type definitions (PLAN §2), and a
//! dock-library major must not invalidate stored workspaces. The client compiles this tree into
//! its dock and maps the dock's state back on every user gesture.
//!
//! Two things are deliberately *not* addressable here: a panel names no device set and no
//! channel. Engine ids are allocated per server run and reused after a restart, so a stored
//! panel pinned to "device set 1" would silently bind to whichever radio opened first — the
//! kind of failure that looks like a working panel. Panels follow the client's active set.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Shape version of a stored [`WorkspaceSnapshot`]. A build refuses to *write back* a snapshot
/// it did not itself produce: layouts are re-persisted on every user gesture, so a downgrade
/// would silently rewrite a newer layout with whatever this build understood of it.
pub const WORKSPACE_SNAPSHOT_VERSION: u32 = 1;

/// Structural caps. A layout is rewritten on every gesture and stored whole, so the bounds are
/// what keeps one row from growing without limit; the numbers are far above any usable station.
pub const MAX_TABS: usize = 32;
pub const MAX_PANELS_PER_TAB: usize = 64;
pub const MAX_SPLIT_DEPTH: usize = 16;
pub const MAX_NAME_LEN: usize = 64;

/// What a panel shows. A closed enum on purpose: the web UI ships inside the same binary as
/// this crate, so client and server can never disagree about the set — and a stored workspace
/// naming a kind this build does not have must fail loudly (the row is refused) rather than be
/// silently rewritten without the panels it could not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    /// Spectrum + waterfall for the active device set.
    Spectrum,
    /// Channel list and per-channel controls.
    Channels,
    /// Frequency scanner.
    Scanner,
    /// Live decoder views for the active set's decoder channels.
    Decoders,
    /// Map of decoded positions.
    Map,
    /// The stored decoder log.
    DecoderLog,
    Presets,
    Bookmarks,
    Templates,
    Recordings,
}

impl PanelKind {
    /// Stable slug, matching the serde representation — used to build panel ids.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spectrum => "spectrum",
            Self::Channels => "channels",
            Self::Scanner => "scanner",
            Self::Decoders => "decoders",
            Self::Map => "map",
            Self::DecoderLog => "decoder_log",
            Self::Presets => "presets",
            Self::Bookmarks => "bookmarks",
            Self::Templates => "templates",
            Self::Recordings => "recordings",
        }
    }
}

/// One panel in a group. `id` is stored rather than derived from `kind`: two spectrum panels in
/// one tab is a legitimate layout, and the dock needs unique panel ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PanelSpec {
    pub id: String,
    pub kind: PanelKind,
    /// User-renamed caption; `None` renders the client's default name for the kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PanelSpec {
    /// A panel with the conventional `panel:<kind>` id, which is what a layout authored in Rust
    /// (defaults, templates) uses.
    #[must_use]
    pub fn new(kind: PanelKind) -> Self {
        Self {
            id: format!("panel:{}", kind.as_str()),
            kind,
            title: None,
        }
    }
}

/// A tab-stack of panels sharing one rectangle.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct PanelGroup {
    pub panels: Vec<PanelSpec>,
    /// Which panel is on top. An id, never an index: an index goes stale the moment a panel is
    /// closed. `None` means "the first one".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

impl PanelGroup {
    /// A group holding one panel per kind, in order, with the first on top.
    #[must_use]
    pub fn of(kinds: &[PanelKind]) -> Self {
        Self {
            panels: kinds.iter().copied().map(PanelSpec::new).collect(),
            active: None,
        }
    }
}

/// How a split arranges its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    /// Children side by side, left to right.
    Row,
    /// Children stacked, top to bottom.
    Column,
}

/// One child of a split, with its share of the parent's extent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LayoutChild {
    /// Share of the parent along its axis, in permille. Integers, not fractions: the layout is
    /// re-persisted on every gesture, and a float would drift across load→save cycles (and can
    /// serialize as `null` for NaN, which fails the *whole* snapshot on the way back in).
    pub weight_permille: u16,
    /// The subtree. `no_recursion` breaks the schema cycle — utoipa would otherwise recurse
    /// forever collecting `LayoutNode` → `LayoutChild` → `LayoutNode`.
    #[schema(no_recursion)]
    pub node: LayoutNode,
}

/// A split and its children.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SplitNode {
    pub direction: SplitDirection,
    pub children: Vec<LayoutChild>,
}

/// The panel-layout tree of one tab. Adjacently tagged like [`crate::ChannelParams`], so the
/// generated TypeScript is a union the client can exhaustively switch on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "node", content = "data", rename_all = "snake_case")]
pub enum LayoutNode {
    Split(SplitNode),
    Group(PanelGroup),
}

impl LayoutNode {
    /// A split whose children are given as `(weight_permille, node)`; weights are stored as
    /// written and normalized on the way into the dock.
    #[must_use]
    pub fn split(direction: SplitDirection, children: Vec<(u16, Self)>) -> Self {
        Self::Split(SplitNode {
            direction,
            children: children
                .into_iter()
                .map(|(weight_permille, node)| LayoutChild {
                    weight_permille,
                    node,
                })
                .collect(),
        })
    }

    /// A leaf holding one panel per kind.
    #[must_use]
    pub fn group(kinds: &[PanelKind]) -> Self {
        Self::Group(PanelGroup::of(kinds))
    }

    fn walk_panels<'a>(&'a self, out: &mut Vec<&'a PanelSpec>) {
        match self {
            Self::Split(split) => {
                for child in &split.children {
                    child.node.walk_panels(out);
                }
            }
            Self::Group(group) => out.extend(group.panels.iter()),
        }
    }

    fn depth(&self) -> usize {
        match self {
            Self::Split(split) => {
                1 + split
                    .children
                    .iter()
                    .map(|c| c.node.depth())
                    .max()
                    .unwrap_or(0)
            }
            Self::Group(_) => 1,
        }
    }

    fn validate(&self, seen: &mut Vec<String>) -> Result<(), WorkspaceError> {
        match self {
            Self::Split(split) => {
                // A one-child split is a node the dock never produces and cannot represent; it
                // means the reverse mapper failed to collapse, so it is rejected rather than
                // stored and half-applied.
                if split.children.len() < 2 {
                    return Err(WorkspaceError::DegenerateSplit);
                }
                for child in &split.children {
                    if child.weight_permille == 0 {
                        return Err(WorkspaceError::ZeroWeight);
                    }
                    child.node.validate(seen)?;
                }
                Ok(())
            }
            Self::Group(group) => {
                for panel in &group.panels {
                    if panel.id.is_empty() || panel.id.len() > MAX_NAME_LEN {
                        return Err(WorkspaceError::PanelId(panel.id.clone()));
                    }
                    if seen.contains(&panel.id) {
                        return Err(WorkspaceError::DuplicatePanel(panel.id.clone()));
                    }
                    seen.push(panel.id.clone());
                }
                match &group.active {
                    Some(active) if !group.panels.iter().any(|p| p.id == *active) => {
                        Err(WorkspaceError::UnknownPanel(active.clone()))
                    }
                    _ => Ok(()),
                }
            }
        }
    }
}

/// A group floating above the grid (PLAN §10). Geometry is a fraction of the dock, never
/// pixels: the same workspace opens on a phone and on a 4K desktop, and stored pixels would put
/// a floating panel off-screen on the smaller one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FloatingGroup {
    pub group: PanelGroup,
    pub x_frac: f32,
    pub y_frac: f32,
    pub w_frac: f32,
    pub h_frac: f32,
}

/// One tab: a named panel layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TabSpec {
    pub id: String,
    pub name: String,
    pub layout: LayoutNode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floating: Vec<FloatingGroup>,
}

/// The stored body of a workspace (PLAN §11: one JSON snapshot per row, like presets — it is
/// written atomically, read whole, and never queried by inner field).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceSnapshot {
    /// [`WORKSPACE_SNAPSHOT_VERSION`] at the time of writing.
    pub version: u32,
    pub tabs: Vec<TabSpec>,
    /// Id of the tab on top; `None` means the first one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<String>,
}

/// Why a snapshot was refused. Structural only — the checks are pure, so they run in `wire`
/// and the server has one rejection point instead of scattered guards. `Display` is written out
/// rather than derived because this crate carries no error-derive dependency (PLAN §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    Version(u32),
    NoTabs,
    TooManyTabs,
    TooManyPanels(String),
    TooDeep,
    DegenerateSplit,
    ZeroWeight,
    PanelId(String),
    DuplicatePanel(String),
    UnknownPanel(String),
    TabId(String),
    DuplicateTab(String),
    UnknownTab(String),
    FloatingGeometry,
    Name,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version(v) => write!(
                f,
                "unsupported workspace snapshot version {v} (this build writes \
                 {WORKSPACE_SNAPSHOT_VERSION})"
            ),
            Self::NoTabs => f.write_str("a workspace needs at least one tab"),
            Self::TooManyTabs => write!(f, "too many tabs (max {MAX_TABS})"),
            Self::TooManyPanels(tab) => {
                write!(f, "too many panels in tab {tab} (max {MAX_PANELS_PER_TAB})")
            }
            Self::TooDeep => write!(f, "layout nested deeper than {MAX_SPLIT_DEPTH}"),
            Self::DegenerateSplit => f.write_str("a split needs at least two children"),
            Self::ZeroWeight => f.write_str("a split child cannot have zero weight"),
            Self::PanelId(id) => write!(f, "invalid panel id {id:?}"),
            Self::DuplicatePanel(id) => write!(f, "duplicate panel id {id}"),
            Self::UnknownPanel(id) => write!(f, "active panel {id} is not in its group"),
            Self::TabId(id) => write!(f, "invalid tab id {id:?}"),
            Self::DuplicateTab(id) => write!(f, "duplicate tab id {id}"),
            Self::UnknownTab(id) => write!(f, "active tab {id} does not exist"),
            Self::FloatingGeometry => f.write_str("floating geometry outside the dock"),
            Self::Name => write!(f, "name must be 1..={MAX_NAME_LEN} characters"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl WorkspaceSnapshot {
    /// Structural bounds a stored layout must satisfy. Client-built trees come from a dock's
    /// own state, so a malformed one is a client bug or a corrupt row — either way it is
    /// refused at the API edge rather than half-applied.
    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.version != WORKSPACE_SNAPSHOT_VERSION {
            return Err(WorkspaceError::Version(self.version));
        }
        if self.tabs.is_empty() {
            return Err(WorkspaceError::NoTabs);
        }
        if self.tabs.len() > MAX_TABS {
            return Err(WorkspaceError::TooManyTabs);
        }
        let mut tab_ids: Vec<&str> = Vec::with_capacity(self.tabs.len());
        for tab in &self.tabs {
            if tab.id.is_empty() || tab.id.len() > MAX_NAME_LEN {
                return Err(WorkspaceError::TabId(tab.id.clone()));
            }
            if tab.name.is_empty() || tab.name.chars().count() > MAX_NAME_LEN {
                return Err(WorkspaceError::Name);
            }
            if tab_ids.contains(&tab.id.as_str()) {
                return Err(WorkspaceError::DuplicateTab(tab.id.clone()));
            }
            tab_ids.push(&tab.id);
            if tab.layout.depth() > MAX_SPLIT_DEPTH {
                return Err(WorkspaceError::TooDeep);
            }
            // Panel ids are unique per tab, not per workspace: two tabs each showing a spectrum
            // is normal, and the dock that hosts them is per tab.
            let mut seen = Vec::new();
            tab.layout.validate(&mut seen)?;
            for floating in &tab.floating {
                if !(floating.x_frac.is_finite()
                    && floating.y_frac.is_finite()
                    && (0.0..=1.0).contains(&floating.w_frac)
                    && (0.0..=1.0).contains(&floating.h_frac)
                    && floating.w_frac > 0.0
                    && floating.h_frac > 0.0)
                {
                    return Err(WorkspaceError::FloatingGeometry);
                }
                LayoutNode::Group(floating.group.clone()).validate(&mut seen)?;
            }
            if seen.len() > MAX_PANELS_PER_TAB {
                return Err(WorkspaceError::TooManyPanels(tab.id.clone()));
            }
        }
        match &self.active_tab {
            Some(active) if !tab_ids.contains(&active.as_str()) => {
                Err(WorkspaceError::UnknownTab(active.clone()))
            }
            _ => Ok(()),
        }
    }

    /// Every panel in the workspace, docked and floating, in tab order.
    #[must_use]
    pub fn panels(&self) -> Vec<&PanelSpec> {
        let mut out = Vec::new();
        for tab in &self.tabs {
            tab.layout.walk_panels(&mut out);
            for floating in &tab.floating {
                out.extend(floating.group.panels.iter());
            }
        }
        out
    }

    /// Insert `tab`, replacing any tab with the same id, and make it active. This is how a
    /// template's layout lands in the active workspace: the id is derived from the template, so
    /// applying it twice replaces its tab instead of stacking copies.
    pub fn upsert_tab(&mut self, tab: TabSpec) {
        self.active_tab = Some(tab.id.clone());
        match self.tabs.iter_mut().find(|t| t.id == tab.id) {
            Some(existing) => *existing = tab,
            None => self.tabs.push(tab),
        }
    }

    /// The layout a fresh install starts on: the fixed arrangement M0–M5 shipped, expressed as
    /// tabs so nothing a user could reach before M6 is now unreachable.
    #[must_use]
    pub fn station_default() -> Self {
        use PanelKind::{
            Bookmarks, Channels, DecoderLog, Decoders, Map, Presets, Recordings, Scanner, Templates,
        };
        Self {
            version: WORKSPACE_SNAPSHOT_VERSION,
            tabs: vec![
                TabSpec {
                    id: "station".to_string(),
                    name: "Station".to_string(),
                    layout: LayoutNode::split(
                        SplitDirection::Column,
                        vec![
                            (600, LayoutNode::group(&[PanelKind::Spectrum])),
                            (
                                400,
                                LayoutNode::split(
                                    SplitDirection::Row,
                                    vec![
                                        (600, LayoutNode::group(&[Channels, Scanner])),
                                        (
                                            400,
                                            LayoutNode::group(&[
                                                Presets, Bookmarks, Templates, Recordings,
                                            ]),
                                        ),
                                    ],
                                ),
                            ),
                        ],
                    ),
                    floating: Vec::new(),
                },
                TabSpec {
                    id: "decoders".to_string(),
                    name: "Decoders".to_string(),
                    layout: LayoutNode::split(
                        SplitDirection::Row,
                        vec![
                            (600, LayoutNode::group(&[Decoders])),
                            (
                                400,
                                LayoutNode::split(
                                    SplitDirection::Column,
                                    vec![
                                        (600, LayoutNode::group(&[Map])),
                                        (400, LayoutNode::group(&[DecoderLog])),
                                    ],
                                ),
                            ),
                        ],
                    ),
                    floating: Vec::new(),
                },
            ],
            active_tab: Some("station".to_string()),
        }
    }
}

/// `GET /api/workspaces` list entry — the projection a switcher needs, without the layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceInfo {
    pub id: i64,
    pub name: String,
    /// RFC3339 UTC.
    pub created_at: String,
    /// RFC3339 UTC.
    pub updated_at: String,
    /// Bumped on every stored change. An update carrying a stale revision is refused rather
    /// than silently overwriting another client's layout.
    pub revision: u64,
    /// Tab count, so the switcher can describe a workspace without fetching its layout.
    pub tabs: u32,
}

/// `GET /api/workspaces`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceInfo>,
    /// The active workspace, or `None` when the last one was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<i64>,
}

/// `GET /api/workspaces/{id}` — the row plus its layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceDetail {
    #[serde(flatten)]
    pub info: WorkspaceInfo,
    pub snapshot: WorkspaceSnapshot,
}

/// `POST /api/workspaces`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    /// Layout to start from; omitted means [`WorkspaceSnapshot::station_default`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

/// `PUT /api/workspaces/{id}` — rename, re-layout, or both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkspaceRequest {
    /// The revision the client last saw. A mismatch is a `409`, never a silent overwrite.
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot::station_default()
    }

    /// The tags the generated TS union switches on.
    #[test]
    fn layout_node_is_adjacently_tagged() {
        let node = LayoutNode::split(
            SplitDirection::Row,
            vec![
                (700, LayoutNode::group(&[PanelKind::Spectrum])),
                (300, LayoutNode::group(&[PanelKind::DecoderLog])),
            ],
        );
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(json["node"], "split");
        assert_eq!(json["data"]["direction"], "row");
        assert_eq!(json["data"]["children"][0]["weight_permille"], 700);
        assert_eq!(json["data"]["children"][0]["node"]["node"], "group");
        assert_eq!(
            json["data"]["children"][1]["node"]["data"]["panels"][0]["kind"],
            "decoder_log"
        );
        assert!(
            json["data"]["children"][0]["node"]["data"]
                .get("active")
                .is_none()
        );
        let back: LayoutNode = serde_json::from_value(json).unwrap();
        assert_eq!(back, node);
    }

    #[test]
    fn station_default_validates_and_holds_every_panel_kind_once() {
        let snap = snapshot();
        snap.validate().expect("default is valid");
        let mut kinds: Vec<PanelKind> = snap.panels().iter().map(|p| p.kind).collect();
        let total = kinds.len();
        kinds.sort_unstable_by_key(|k| k.as_str());
        kinds.dedup();
        assert_eq!(kinds.len(), total, "a kind appears twice in the default");
        assert_eq!(total, 10, "every panel kind should be reachable by default");
    }

    #[test]
    fn snapshot_roundtrips_through_json() {
        let snap = snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let back: WorkspaceSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn validate_rejects_structural_nonsense() {
        let mut wrong_version = snapshot();
        wrong_version.version = 99;
        assert_eq!(wrong_version.validate(), Err(WorkspaceError::Version(99)));

        let empty = WorkspaceSnapshot {
            version: WORKSPACE_SNAPSHOT_VERSION,
            tabs: Vec::new(),
            active_tab: None,
        };
        assert_eq!(empty.validate(), Err(WorkspaceError::NoTabs));

        let one_child = WorkspaceSnapshot {
            version: WORKSPACE_SNAPSHOT_VERSION,
            tabs: vec![TabSpec {
                id: "t".to_string(),
                name: "T".to_string(),
                layout: LayoutNode::split(
                    SplitDirection::Row,
                    vec![(1000, LayoutNode::group(&[PanelKind::Map]))],
                ),
                floating: Vec::new(),
            }],
            active_tab: None,
        };
        assert_eq!(one_child.validate(), Err(WorkspaceError::DegenerateSplit));

        let mut duplicate = snapshot();
        duplicate.tabs[1].id = duplicate.tabs[0].id.clone();
        assert!(matches!(
            duplicate.validate(),
            Err(WorkspaceError::DuplicateTab(_))
        ));

        let mut unknown_tab = snapshot();
        unknown_tab.active_tab = Some("nope".to_string());
        assert_eq!(
            unknown_tab.validate(),
            Err(WorkspaceError::UnknownTab("nope".to_string()))
        );

        let mut zero_weight = snapshot();
        if let LayoutNode::Split(split) = &mut zero_weight.tabs[0].layout {
            split.children[0].weight_permille = 0;
        }
        assert_eq!(zero_weight.validate(), Err(WorkspaceError::ZeroWeight));
    }

    /// Two panels with the same id in one tab would collide in the dock, which keys panels by
    /// id — one of them would silently disappear.
    #[test]
    fn validate_rejects_duplicate_panel_ids_within_a_tab() {
        let mut snap = snapshot();
        snap.tabs[0].layout = LayoutNode::split(
            SplitDirection::Row,
            vec![
                (500, LayoutNode::group(&[PanelKind::Map])),
                (500, LayoutNode::group(&[PanelKind::Map])),
            ],
        );
        assert!(matches!(
            snap.validate(),
            Err(WorkspaceError::DuplicatePanel(_))
        ));

        // The same id in a *different* tab is fine: each tab is its own dock.
        let ok = snapshot();
        assert!(
            ok.panels()
                .iter()
                .filter(|p| p.kind == PanelKind::Map)
                .count()
                <= 1
        );
    }

    #[test]
    fn validate_rejects_a_floating_group_outside_the_dock() {
        let mut snap = snapshot();
        snap.tabs[0].floating = vec![FloatingGroup {
            group: PanelGroup::of(&[PanelKind::Scanner]),
            x_frac: 0.1,
            y_frac: 0.1,
            w_frac: 0.0,
            h_frac: 0.4,
        }];
        assert_eq!(snap.validate(), Err(WorkspaceError::FloatingGeometry));

        // A floating panel duplicating a docked id collides just the same.
        let mut collide = snapshot();
        collide.tabs[0].floating = vec![FloatingGroup {
            group: PanelGroup::of(&[PanelKind::Channels]),
            x_frac: 0.1,
            y_frac: 0.1,
            w_frac: 0.3,
            h_frac: 0.3,
        }];
        assert!(matches!(
            collide.validate(),
            Err(WorkspaceError::DuplicatePanel(_))
        ));
    }

    #[test]
    fn upsert_tab_replaces_in_place_and_activates() {
        let mut snap = snapshot();
        let before = snap.tabs.len();
        let tab = TabSpec {
            id: "template:adsb".to_string(),
            name: "Aircraft".to_string(),
            layout: LayoutNode::group(&[PanelKind::Decoders]),
            floating: Vec::new(),
        };
        snap.upsert_tab(tab.clone());
        assert_eq!(snap.tabs.len(), before + 1);
        assert_eq!(snap.active_tab.as_deref(), Some("template:adsb"));

        snap.active_tab = Some("station".to_string());
        let mut renamed = tab;
        renamed.name = "Aircraft (ADS-B)".to_string();
        snap.upsert_tab(renamed);
        assert_eq!(
            snap.tabs.len(),
            before + 1,
            "re-apply must replace, not add"
        );
        assert_eq!(snap.tabs[before].name, "Aircraft (ADS-B)");
        assert_eq!(snap.active_tab.as_deref(), Some("template:adsb"));
        snap.validate().expect("still valid");
    }

    /// `active` names a panel id; a stale one would leave the dock without a visible tab.
    #[test]
    fn validate_rejects_an_active_panel_that_is_not_in_its_group() {
        let snap = WorkspaceSnapshot {
            version: WORKSPACE_SNAPSHOT_VERSION,
            tabs: vec![TabSpec {
                id: "t".to_string(),
                name: "T".to_string(),
                layout: LayoutNode::Group(PanelGroup {
                    panels: vec![PanelSpec::new(PanelKind::Map)],
                    active: Some("panel:spectrum".to_string()),
                }),
                floating: Vec::new(),
            }],
            active_tab: None,
        };
        assert_eq!(
            snap.validate(),
            Err(WorkspaceError::UnknownPanel("panel:spectrum".to_string()))
        );
    }
}
