fn main() {
    println!("cargo:rerun-if-changed=resources/soapy");
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Resources/soapy/lib");
    #[cfg(target_os = "linux")]
    println!(
        "cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN/../lib/sdr--/soapy/lib,-rpath,$ORIGIN/soapy/lib"
    );
    tauri_build::build();
}
