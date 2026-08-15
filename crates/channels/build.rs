fn main() {
    build_dstar_vocoder();
    build_fdmdv();
    if std::env::var("CARGO_CFG_UNIX").is_ok() {
        println!("cargo:rustc-link-lib=m");
    }
}

fn build_dstar_vocoder() {
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
}

fn build_fdmdv() {
    let sources = [
        "vendor/codec2-fdmdv/fdmdv.c",
        "vendor/codec2-fdmdv/codec2_fft.c",
        "vendor/codec2-fdmdv/kiss_fft.c",
        "vendor/codec2-fdmdv/kiss_fftr.c",
        "vendor/codec2-fdmdv/freedv_wrapper.c",
    ];
    let mut build = cc::Build::new();
    build
        .include("vendor/codec2-fdmdv")
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
        println!("cargo:rerun-if-changed=vendor/codec2-fdmdv/{file}");
    }
    build.compile("sdrmm_fdmdv");
}
