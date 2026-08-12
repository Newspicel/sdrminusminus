fn main() {
    let sources = [
        "vendor/mbelib/ecc.c",
        "vendor/mbelib/mbelib.c",
        "vendor/mbelib/ambe3600x2400.c",
        "vendor/mbelib/dstar_wrapper.c",
    ];
    let mut build = cc::Build::new();
    build
        .include("vendor/mbelib")
        .define("_USE_MATH_DEFINES", None)
        .warnings(false);
    for source in sources {
        println!("cargo:rerun-if-changed={source}");
        build.file(source);
    }
    for header in [
        "vendor/mbelib/COPYRIGHT",
        "vendor/mbelib/config.h",
        "vendor/mbelib/mbelib.h",
        "vendor/mbelib/mbelib_const.h",
        "vendor/mbelib/ecc_const.h",
        "vendor/mbelib/ambe3600x2400_const.h",
    ] {
        println!("cargo:rerun-if-changed={header}");
    }
    build.compile("sdrmm_dstar_ambe");
    if std::env::var("CARGO_CFG_UNIX").is_ok() {
        println!("cargo:rustc-link-lib=m");
    }
}
