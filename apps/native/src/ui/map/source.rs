use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{
    geo::TileId,
    mvt,
    paint::{self, Painted},
    pmtiles::{self, Entry, Found, Header},
};

pub const ONLINE_TILES: &str = "https://tiles.openfreemap.org/planet";
pub const OFFLINE_PATH: &str = "/api/basemap.pmtiles";
pub const TIMEOUT: Duration = Duration::from_secs(4);
const TILE_TIMEOUT: Duration = Duration::from_secs(20);
const ONLINE_MAX_ZOOM: u8 = 14;
const LEAVES_KEPT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pending,
    Online,
    Offline,
    Blank,
}

pub struct Archive {
    pub url: String,
    pub header: Header,
    pub root: Vec<Entry>,
    leaves: Mutex<VecDeque<(u64, Arc<Vec<Entry>>)>>,
}

#[derive(Clone)]
pub enum Basemap {
    Online { template: String, max_zoom: u8 },
    Offline(Arc<Archive>),
    Blank,
}

impl Basemap {
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Online { .. } => Kind::Online,
            Self::Offline(_) => Kind::Offline,
            Self::Blank => Kind::Blank,
        }
    }

    #[must_use]
    pub fn max_zoom(&self) -> u8 {
        match self {
            Self::Online { max_zoom, .. } => *max_zoom,
            Self::Offline(archive) => archive.header.max_zoom,
            Self::Blank => 0,
        }
    }

    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Online { template, .. } => template,
            Self::Offline(archive) => &archive.url,
            Self::Blank => "",
        }
    }

    #[must_use]
    pub const fn attribution(&self) -> &'static str {
        match self {
            Self::Online { .. } => "OpenFreeMap © OpenMapTiles © OpenStreetMap",
            Self::Offline(_) => "© OpenStreetMap",
            Self::Blank => "",
        }
    }
}

#[must_use]
pub fn choose(online: Option<Basemap>, offline: Option<Arc<Archive>>) -> Basemap {
    online
        .or_else(|| offline.map(Basemap::Offline))
        .unwrap_or(Basemap::Blank)
}

#[derive(Clone)]
pub struct Net {
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct TileJson {
    tiles: Vec<String>,
    #[serde(default)]
    maxzoom: Option<u8>,
}

impl Net {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("sdrmm-native/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("cannot build the map client")?,
        })
    }

    async fn get(&self, url: &str, timeout: Duration) -> anyhow::Result<Vec<u8>> {
        let response = self
            .http
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .context(url.to_owned())?;
        let status = response.status();
        if !status.is_success() {
            bail!("{url}: {status}");
        }
        Ok(response
            .bytes()
            .await
            .context("cannot read a map response")?
            .to_vec())
    }

    async fn range(&self, url: &str, offset: u64, length: u64) -> anyhow::Result<Vec<u8>> {
        let last = offset + length.max(1) - 1;
        let response = self
            .http
            .get(url)
            .header(reqwest::header::RANGE, format!("bytes={offset}-{last}"))
            .timeout(TILE_TIMEOUT)
            .send()
            .await
            .context(url.to_owned())?;
        let status = response.status();
        if !status.is_success() {
            bail!("{url}: {status}");
        }
        let bytes = response.bytes().await.context("cannot read the basemap")?;
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            return Ok(bytes.to_vec());
        }
        let start = usize::try_from(offset)?;
        let end = usize::try_from(offset + length)?.min(bytes.len());
        Ok(bytes.get(start..end).unwrap_or_default().to_vec())
    }

    pub async fn online(&self) -> Option<Basemap> {
        let bytes = self.get(ONLINE_TILES, TIMEOUT).await.ok()?;
        let described: TileJson = serde_json::from_slice(&bytes).ok()?;
        let template = described.tiles.into_iter().next()?;
        Some(Basemap::Online {
            template,
            max_zoom: described
                .maxzoom
                .unwrap_or(ONLINE_MAX_ZOOM)
                .min(ONLINE_MAX_ZOOM),
        })
    }

    pub async fn offline(&self, url: String) -> Option<Arc<Archive>> {
        match self.open_archive(url).await {
            Ok(archive) => Some(Arc::new(archive)),
            Err(error) => {
                tracing::debug!(%error, "no offline basemap");
                None
            }
        }
    }

    async fn open_archive(&self, url: String) -> anyhow::Result<Archive> {
        let head = self.range(&url, 0, pmtiles::FIRST_READ).await?;
        let header = pmtiles::header(&head)?;
        if header.tile_type != pmtiles::MVT_TILES {
            bail!(
                "the basemap holds tile type {}, not vector tiles",
                header.tile_type
            );
        }
        let start = usize::try_from(header.root_offset)?;
        let end = start + usize::try_from(header.root_length)?;
        let raw = match head.get(start..end) {
            Some(raw) => raw.to_vec(),
            None => {
                self.range(&url, header.root_offset, header.root_length)
                    .await?
            }
        };
        let root = pmtiles::directory(&header.internal.inflate(&raw)?)?;
        Ok(Archive {
            url,
            header,
            root,
            leaves: Mutex::new(VecDeque::new()),
        })
    }

    pub async fn discover(&self, offline_url: String) -> Basemap {
        let online = self.online().await;
        let offline = if online.is_none() {
            self.offline(offline_url).await
        } else {
            None
        };
        choose(online, offline)
    }

    pub async fn tile(&self, basemap: &Basemap, id: TileId) -> anyhow::Result<Painted> {
        let bytes = match basemap {
            Basemap::Online { template, .. } => {
                let url = template
                    .replace("{z}", &id.z.to_string())
                    .replace("{x}", &id.x.to_string())
                    .replace("{y}", &id.y.to_string());
                self.get(&url, TILE_TIMEOUT).await?
            }
            Basemap::Offline(archive) => match self.archived(archive, id).await? {
                Some(bytes) => archive.header.tiles.inflate(&bytes)?,
                None => return Ok(Painted::default()),
            },
            Basemap::Blank => return Ok(Painted::default()),
        };
        let bytes = if pmtiles::gzipped(&bytes) {
            pmtiles::gunzip(&bytes)?
        } else {
            bytes
        };
        Ok(paint::paint(&mvt::decode(&bytes)?, id))
    }

    async fn archived(&self, archive: &Archive, id: TileId) -> anyhow::Result<Option<Vec<u8>>> {
        let wanted = pmtiles::tile_id(id);
        let mut entries = Arc::new(archive.root.clone());
        for _ in 0..pmtiles::MAX_DEPTH {
            match pmtiles::find(&entries, wanted) {
                Found::Missing => return Ok(None),
                Found::Tile { offset, length } => {
                    let at = archive.header.data_offset + offset;
                    return self
                        .range(&archive.url, at, u64::from(length))
                        .await
                        .map(Some);
                }
                Found::Leaf { offset, length } => {
                    entries = self.leaf(archive, offset, length).await?;
                }
            }
        }
        bail!("the basemap directory nests too deep")
    }

    async fn leaf(
        &self,
        archive: &Archive,
        offset: u64,
        length: u32,
    ) -> anyhow::Result<Arc<Vec<Entry>>> {
        if let Some(held) = cached_leaf(archive, offset) {
            return Ok(held);
        }
        let at = archive.header.leaf_offset + offset;
        let raw = self.range(&archive.url, at, u64::from(length)).await?;
        let entries = Arc::new(pmtiles::directory(&archive.header.internal.inflate(&raw)?)?);
        if let Ok(mut leaves) = archive.leaves.lock() {
            leaves.push_back((offset, entries.clone()));
            while leaves.len() > LEAVES_KEPT {
                leaves.pop_front();
            }
        }
        Ok(entries)
    }
}

fn cached_leaf(archive: &Archive, offset: u64) -> Option<Arc<Vec<Entry>>> {
    let leaves = archive.leaves.lock().ok()?;
    leaves
        .iter()
        .find(|(held, _)| *held == offset)
        .map(|(_, entries)| entries.clone())
}

pub enum Slot {
    Pending,
    Ready(Arc<Painted>),
    Failed,
}

pub struct Cache {
    slots: HashMap<(String, TileId), Slot>,
    order: VecDeque<(String, TileId)>,
    capacity: usize,
}

impl Cache {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    #[must_use]
    pub fn ready(&self, source: &str, id: TileId) -> Option<Arc<Painted>> {
        match self.slots.get(&(source.to_owned(), id)) {
            Some(Slot::Ready(painted)) => Some(painted.clone()),
            _ => None,
        }
    }

    #[must_use]
    pub fn claim(&mut self, source: &str, id: TileId) -> bool {
        let key = (source.to_owned(), id);
        if self.slots.contains_key(&key) {
            return false;
        }
        self.slots.insert(key.clone(), Slot::Pending);
        self.order.push_back(key);
        self.evict();
        true
    }

    pub fn settle(&mut self, source: &str, id: TileId, slot: Slot) {
        let key = (source.to_owned(), id);
        if let Some(held) = self.slots.get_mut(&key) {
            *held = slot;
        }
    }

    fn evict(&mut self) {
        while self.order.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                return;
            };
            if matches!(self.slots.get(&oldest), Some(Slot::Pending)) {
                self.order.push_back(oldest);
                return;
            }
            self.slots.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::map::pmtiles::Compression;

    fn archive() -> Arc<Archive> {
        Arc::new(Archive {
            url: OFFLINE_PATH.to_owned(),
            header: Header {
                root_offset: 127,
                root_length: 0,
                leaf_offset: 0,
                data_offset: 0,
                internal: Compression::None,
                tiles: Compression::Gzip,
                tile_type: pmtiles::MVT_TILES,
                min_zoom: 0,
                max_zoom: 14,
            },
            root: Vec::new(),
            leaves: Mutex::new(VecDeque::new()),
        })
    }

    fn online() -> Basemap {
        Basemap::Online {
            template: "https://tiles/{z}/{x}/{y}.pbf".to_owned(),
            max_zoom: 14,
        }
    }

    #[test]
    fn the_online_basemap_wins_whenever_one_came_back() {
        assert_eq!(choose(Some(online()), Some(archive())).kind(), Kind::Online);
    }

    #[test]
    fn the_operators_own_archive_is_the_fallback() {
        let chosen = choose(None, Some(archive()));
        assert_eq!(chosen.kind(), Kind::Offline);
        assert_eq!(chosen.key(), OFFLINE_PATH);
    }

    #[test]
    fn with_nothing_to_draw_the_map_says_so() {
        let chosen = choose(None, None);
        assert_eq!(chosen.kind(), Kind::Blank);
        assert_eq!(chosen.attribution(), "");
    }

    #[test]
    fn a_tile_is_fetched_once_and_old_tiles_make_room() {
        let mut cache = Cache::new(2);
        let tile = |x| TileId { z: 3, x, y: 1 };
        assert!(cache.claim("a", tile(0)));
        assert!(!cache.claim("a", tile(0)));
        cache.settle("a", tile(0), Slot::Ready(Arc::new(Painted::default())));
        assert!(cache.ready("a", tile(0)).is_some());
        assert!(cache.claim("a", tile(1)));
        cache.settle("a", tile(1), Slot::Failed);
        assert!(cache.claim("a", tile(2)));
        assert!(cache.ready("a", tile(0)).is_none());
        assert!(cache.claim("b", tile(0)));
    }
}
