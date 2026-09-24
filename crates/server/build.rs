#![allow(clippy::expect_used)]

use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use flate2::{Compression, write::GzEncoder};

const WORTHWHILE_RATIO: f64 = 0.9;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let dist = manifest.join("../../web/dist");
    fs::create_dir_all(&dist).expect("create web/dist");
    println!("cargo:rerun-if-changed=../../web/dist");
    println!("cargo:rerun-if-changed=data");

    write_web_table(&dist, &out.join("web"), &out.join("web_assets.rs"));
    compress_tree(&manifest.join("data"), &out.join("data"));
}

fn write_web_table(dist: &Path, out: &Path, table: &Path) {
    let mut files = Vec::new();
    collect(dist, dist, &mut files);
    files.sort();
    let mut source = String::from("&[\n");
    for relative in files {
        let raw = fs::read(dist.join(&relative)).expect("read web asset");
        let packed = gzip(&raw);
        let gzipped = (packed.len() as f64) < raw.len() as f64 * WORTHWHILE_RATIO;
        let stored = out.join(&relative);
        fs::create_dir_all(stored.parent().expect("asset parent")).expect("create asset dir");
        fs::write(&stored, if gzipped { &packed } else { &raw }).expect("write asset");
        let mime = mime_guess::from_path(&relative).first_or_octet_stream();
        writeln!(
            source,
            "    Asset {{ path: {relative:?}, mime: {mime:?}, gzipped: {gzipped}, body: include_bytes!({stored:?}) }},",
            mime = mime.as_ref(),
        )
        .expect("write table row");
    }
    source.push(']');
    fs::write(table, source).expect("write asset table");
}

fn compress_tree(source: &Path, out: &Path) {
    let mut files = Vec::new();
    collect(source, source, &mut files);
    for relative in files {
        let raw = fs::read(source.join(&relative)).expect("read data file");
        let stored = out.join(format!("{relative}.gz"));
        fs::create_dir_all(stored.parent().expect("data parent")).expect("create data dir");
        fs::write(stored, gzip(&raw)).expect("write data file");
    }
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).expect("inside root");
            files.push(relative.to_str().expect("utf-8 path").replace('\\', "/"));
        }
    }
}

fn gzip(raw: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(raw).expect("gzip");
    encoder.finish().expect("gzip")
}
