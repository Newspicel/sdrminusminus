fn main() {
    let sources = [
        "vendor/fdmdv/fdmdv.c",
        "vendor/fdmdv/codec2_fft.c",
        "vendor/fdmdv/kiss_fft.c",
        "vendor/fdmdv/kiss_fftr.c",
        "vendor/fdmdv/freedv_wrapper.c",
    ];
    let mut build = cc::Build::new();
    build
        .include("vendor/fdmdv")
        .define("_USE_MATH_DEFINES", None)
        .warnings(false);
    for source in sources {
        println!("cargo:rerun-if-changed={source}");
        build.file(source);
    }
    for file in [
        "COPYING",
        "README.md",
        "_kiss_fft_guts.h",
        "codec2_fdmdv.h",
        "codec2_fft.h",
        "comp.h",
        "comp_prim.h",
        "debug_alloc.h",
        "defines.h",
        "fdmdv_internal.h",
        "hanning.h",
        "kiss_fft.h",
        "kiss_fftr.h",
        "machdep.h",
        "modem_stats.h",
        "os.h",
        "pilot_coeff.h",
        "rn.h",
        "rxdec_coeff.h",
        "test_bits.h",
    ] {
        println!("cargo:rerun-if-changed=vendor/fdmdv/{file}");
    }
    build.compile("sdrmm_fdmdv");
    if std::env::var("CARGO_CFG_UNIX").is_ok() {
        println!("cargo:rustc-link-lib=m");
    }
}
