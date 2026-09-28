fn bundled_rpath() {
    println!("cargo::rerun-if-env-changed=FFMPEG_DIR");
    if std::env::var_os("FFMPEG_DIR").is_none() {
        return;
    }
    let dirs: &[&str] = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => &[
            "@executable_path",
            "@executable_path/../Frameworks",
            "@executable_path/../lib/sdrmm",
        ],
        Ok("linux") => {
            println!("cargo::rustc-link-arg-bins=-Wl,--disable-new-dtags");
            &[
                "$ORIGIN",
                "$ORIGIN/../lib/sdrmm",
                "$ORIGIN/../lib/sdrminusminus",
            ]
        }
        _ => return,
    };
    for dir in dirs {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,{dir}");
    }
}
