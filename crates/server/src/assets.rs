use axum::{
    Json,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
};
use sdrmm_wire::ApiError;

use crate::packed::inflate;

struct Asset {
    path: &'static str,
    mime: &'static str,
    gzipped: bool,
    body: &'static [u8],
}

static ASSETS: &[Asset] = include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

const API_DOCS: &str = "api-docs.html";

pub(crate) async fn static_handler(uri: Uri, headers: HeaderMap) -> Response {
    let raw = uri.path().trim_start_matches('/');

    if raw == "api" || raw.starts_with("api/") {
        let body = ApiError {
            error: format!("no such endpoint: /{raw}"),
            detail: None,
            code: Some(sdrmm_wire::ErrorCode::NotFound),
        };
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }

    let path = if raw.is_empty() { "index.html" } else { raw };
    match find(path).or_else(|| find("index.html")) {
        Some(asset) => respond(asset, &headers),
        None => (StatusCode::SERVICE_UNAVAILABLE, Html(NOT_BUILT)).into_response(),
    }
}

pub(crate) async fn api_docs(headers: HeaderMap) -> Response {
    match find(API_DOCS) {
        Some(asset) => respond(asset, &headers),
        None => (StatusCode::SERVICE_UNAVAILABLE, Html(NOT_BUILT)).into_response(),
    }
}

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS
        .binary_search_by(|asset| asset.path.cmp(path))
        .ok()
        .map(|index| &ASSETS[index])
}

fn respond(asset: &'static Asset, headers: &HeaderMap) -> Response {
    let content_type = [(header::CONTENT_TYPE, asset.mime)];
    if !asset.gzipped {
        return (content_type, asset.body).into_response();
    }
    let vary = (header::VARY, HeaderValue::from_static("accept-encoding"));
    if accepts_gzip(headers) {
        let encoding = (header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        return (content_type, [encoding, vary], asset.body).into_response();
    }
    match inflate(asset.body) {
        Ok(raw) => (content_type, [vary], raw).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: format!("embedded asset {} is corrupt", asset.path),
                detail: Some(error.to_string()),
                code: None,
            }),
        )
            .into_response(),
    }
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            let refused = parts.any(|param| {
                param
                    .strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
                    .is_some_and(|q| q == 0.0)
            });
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
        })
}

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>SDR--</title>\
<body style=\"font-family:system-ui;background:#0b0e14;color:#c8d3e0;padding:3rem\">\
<h1>SDR-- server is running</h1>\
<p>The web UI has not been built yet. Run <code>cargo xtask dev</code> for the dev server, \
or <code>cargo xtask codegen &amp;&amp; pnpm -C web build</code> to embed it.</p>";

#[cfg(test)]
mod tests {
    use super::*;

    fn with_accept(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn gzip_is_accepted_only_when_offered() {
        assert!(accepts_gzip(&with_accept("gzip, deflate, br")));
        assert!(accepts_gzip(&with_accept("br;q=1.0, GZIP;q=0.5")));
        assert!(accepts_gzip(&with_accept("*")));
        assert!(!accepts_gzip(&with_accept("br, deflate")));
        assert!(!accepts_gzip(&with_accept("gzip;q=0")));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[test]
    fn asset_table_is_sorted_for_lookup() {
        assert!(ASSETS.windows(2).all(|pair| pair[0].path < pair[1].path));
    }

    #[test]
    fn gzipped_assets_inflate() {
        for asset in ASSETS.iter().filter(|asset| asset.gzipped) {
            assert!(inflate(asset.body).is_ok(), "{}", asset.path);
        }
    }
}
