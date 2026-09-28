include!("../bundled_rpath.rs");

fn main() {
    println!("cargo::rerun-if-env-changed=SDRMM_RELEASE");
    bundled_rpath();
    tauri_build::build();
}
