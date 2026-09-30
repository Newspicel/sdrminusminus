use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use crate::{
    SigmfError,
    collection::{collection_path, lane_of, lanes_on_disk, scan_collections},
    data_path, meta_path, scan_stems,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    Recording(PathBuf),
    Collection(PathBuf),
}

impl Stored {
    #[must_use]
    pub fn at(stem: &Path) -> Self {
        if collection_path(stem).is_file() {
            Self::Collection(stem.to_path_buf())
        } else {
            Self::Recording(stem.to_path_buf())
        }
    }

    #[must_use]
    pub fn stem(&self) -> &Path {
        match self {
            Self::Recording(stem) | Self::Collection(stem) => stem,
        }
    }

    #[must_use]
    pub fn shown_file(&self) -> PathBuf {
        match self {
            Self::Recording(stem) => data_path(stem),
            Self::Collection(stem) => collection_path(stem),
        }
    }

    pub fn remove(&self) -> Result<(), SigmfError> {
        match self {
            Self::Recording(stem) => remove_pair(stem),
            Self::Collection(stem) => {
                for lane in lanes_on_disk(stem)? {
                    remove_pair(&lane)?;
                }
                remove_file(&collection_path(stem))
            }
        }
    }
}

pub fn scan_library(dir: &Path) -> Result<Vec<Stored>, SigmfError> {
    let collections = scan_collections(dir)?;
    let mut stored: Vec<Stored> = scan_stems(dir)?
        .into_iter()
        .filter(|stem| {
            collections
                .iter()
                .all(|collection| collection != stem && lane_of(collection, stem).is_none())
        })
        .map(Stored::Recording)
        .collect();
    stored.extend(collections.into_iter().map(Stored::Collection));
    stored.sort_by(|a, b| a.stem().cmp(b.stem()));
    Ok(stored)
}

fn remove_pair(stem: &Path) -> Result<(), SigmfError> {
    remove_file(&meta_path(stem))?;
    remove_file(&data_path(stem))
}

fn remove_file(path: &Path) -> Result<(), SigmfError> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => Err(SigmfError::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
pub(crate) mod tests;
