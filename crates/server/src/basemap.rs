use std::path::PathBuf;

use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;

use crate::AppState;

/// What an operator drops next to the database to make the map work with no internet.
///
/// One file, one name, no configuration: a PMTiles archive is a single seekable file, which is
/// exactly what a range-serving static handler and a browser that reads it back can share.
pub const BASEMAP_FILE: &str = "basemap.pmtiles";

#[must_use]
pub(crate) fn basemap_path(state: &AppState) -> Option<PathBuf> {
    let path = state.db_path.as_ref()?.parent()?.join(BASEMAP_FILE);
    path.is_file().then_some(path)
}

/// Serves the archive with range support, because a PMTiles reader fetches a few kilobytes at a
/// time out of a file that may be gigabytes.
pub(crate) async fn handler(State(state): State<AppState>, request: Request) -> Response {
    let Some(path) = basemap_path(&state) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    match tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await
    {
        Ok(response) => response.into_response(),
        Err(error) => {
            tracing::warn!(%error, "could not serve the offline basemap");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_is_looked_for_beside_the_database() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let db = dir.path().join("sdrmm.db");
        let mut state = crate::AppState::new(
            sdrmm_engine::Engine::with_registry(sdrmm_device::DeviceRegistry::new(), None),
            std::sync::Arc::new(crate::store::Store::open(None).expect("store")),
        );
        state.db_path = Some(db);
        assert!(basemap_path(&state).is_none());
        std::fs::write(dir.path().join(BASEMAP_FILE), b"not really tiles").expect("write");
        assert_eq!(
            basemap_path(&state),
            Some(dir.path().join(BASEMAP_FILE)),
            "an archive beside the database is the one that gets served"
        );
    }

    #[test]
    fn a_server_with_no_database_has_nowhere_to_look() {
        let state = crate::AppState::new(
            sdrmm_engine::Engine::with_registry(sdrmm_device::DeviceRegistry::new(), None),
            std::sync::Arc::new(crate::store::Store::open(None).expect("store")),
        );
        assert!(basemap_path(&state).is_none());
    }
}
