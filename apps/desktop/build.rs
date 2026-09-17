fn main() {
    println!("cargo::rerun-if-env-changed=SDRMM_RELEASE");
    tauri_build::build();
}
