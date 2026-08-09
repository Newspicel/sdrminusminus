//! Embedded UI assets (PLAN §10): the built frontend is baked into the binary via `rust-embed`
//! so a Pi deployment is one file. Unknown non-API paths fall back to `index.html` (SPA
//! routing). Before the frontend is ever built the embed is empty and we serve a hint page.

use axum::{
    Json,
    http::{StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
};
use rust_embed::Embed;
use sdrmm_wire::ApiError;

#[derive(Embed)]
#[folder = "../../web/dist"]
struct Assets;

pub(crate) async fn static_handler(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');

    // An unmatched /api path is a real 404 on the JSON surface — never mask it as the SPA shell
    // (a stale client would parse HTML as JSON). Return a typed error like the REST handlers.
    if raw == "api" || raw.starts_with("api/") {
        let body = ApiError {
            error: format!("no such endpoint: /{raw}"),
            detail: None,
        };
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }

    let path = if raw.is_empty() { "index.html" } else { raw };

    if let Some(file) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response();
    }

    match Assets::get("index.html") {
        Some(index) => ([(header::CONTENT_TYPE, "text/html")], index.data).into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, Html(NOT_BUILT)).into_response(),
    }
}

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>sdr--</title>\
<body style=\"font-family:system-ui;background:#0b0e14;color:#c8d3e0;padding:3rem\">\
<h1>sdr-- server is running</h1>\
<p>The web UI has not been built yet. Run <code>cargo xtask dev</code> for the dev server, \
or <code>cargo xtask codegen &amp;&amp; pnpm -C web build</code> to embed it.</p>";
