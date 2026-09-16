fn main() {
    build_dstar_vocoder();
    build_fdmdv();
    link_whole_media_util();
    if std::env::var("CARGO_CFG_UNIX").is_ok() {
        println!("cargo:rustc-link-lib=m");
    }
}

// GNU ld reads archives once, in order, keeping only the members something has already asked for.
// ffmpeg-sys names avutil ahead of the libraries that call into it and rustc names this crate's
// libraries ahead of that, so avutil is always read before avcodec asks for anything in it and
// every call into it comes out undefined. Taking the whole archive settles that wherever avutil
// lands. The Apple and MSVC linkers resolve archives in any order and need none of this.
fn link_whole_media_util() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let Ok(prefix) = std::env::var("FFMPEG_DIR") else {
        return;
    };
    let lib = std::path::Path::new(&prefix).join("lib");
    if !lib.join("libavutil.a").is_file() {
        return;
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=avutil");
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
