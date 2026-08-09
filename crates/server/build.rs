//! Ensure the embedded-UI directory exists before `rust-embed` expands, so the server crate
//! compiles on a fresh clone before the frontend has ever been built. Real builds populate
//! `web/dist` via `cargo xtask` and the assets are embedded in release binaries.
#![allow(clippy::expect_used)]

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dist = std::path::Path::new(&manifest).join("../../web/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed=../../web/dist");
}
