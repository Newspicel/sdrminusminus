#![allow(clippy::expect_used)]

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dist = std::path::Path::new(&manifest).join("../../web/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed=../../web/dist");
}
