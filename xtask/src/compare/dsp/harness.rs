use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};

use crate::compare::report::{self, SELF};

const SOURCES: &str = "xtask/compare/dsp";
const OUT: &str = "target/compare/dsp";
const RUST_OUT: &str = "target/compare/dsp-rust";
const GNURADIO_MODULES: &[&str] = &[
    "gnuradio-filter",
    "gnuradio-analog",
    "gnuradio-blocks",
    "gnuradio-fft",
    "gnuradio-runtime",
    "volk",
    "spdlog",
];

pub struct Timing {
    pub block: usize,
    pub reps: usize,
    pub rep_seconds: f64,
}

impl Timing {
    fn args(&self) -> [String; 3] {
        [
            self.block.to_string(),
            self.reps.to_string(),
            self.rep_seconds.to_string(),
        ]
    }
}

pub struct Built {
    pub name: &'static str,
    pub version: Option<String>,
    program: PathBuf,
}

impl Built {
    pub fn execute(&self, timing: &Timing) -> Result<String> {
        let out = Command::new(&self.program)
            .args(timing.args())
            .output()
            .with_context(|| format!("run {}", self.program.display()))?;
        ensure!(
            out.status.success(),
            "{} failed with {}: {}",
            self.name,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8(out.stdout)?)
    }
}

pub fn build(root: &Path) -> Result<Vec<Built>> {
    let out = root.join(OUT);
    std::fs::create_dir_all(&out).with_context(|| format!("create {}", out.display()))?;
    let rust = build_rust(root)?;
    let lock = std::fs::read_to_string(root.join(SOURCES).join("rust/Cargo.lock"))
        .context("read the harness Cargo.lock")?;
    let futuredsp = lock_version(&lock, "futuredsp").context("futuredsp is not locked")?;
    let compilers = Compilers::detect()?;
    Ok(vec![
        own(root, &rust)?,
        compilers.gnuradio(root)?,
        compilers.c(root, "liquid-dsp", "liquid", &["-lliquid", "-lm"])?,
        Built {
            name: "FutureSDR",
            version: Some(format!("futuredsp {futuredsp}")),
            program: rust.join("futuresdr"),
        },
        compilers.c(root, "FFTW", "fftw", &["-lfftw3f", "-lm"])?,
    ])
}

pub fn ours(root: &Path) -> Result<Built> {
    own(root, &build_rust(root)?)
}

fn own(root: &Path, rust: &Path) -> Result<Built> {
    Ok(Built {
        name: SELF,
        version: Some(report::version(root)?),
        program: rust.join("sdrmm"),
    })
}

fn build_rust(root: &Path) -> Result<PathBuf> {
    let manifest = root.join(SOURCES).join("rust/Cargo.toml");
    let target = root.join(RUST_OUT);
    crate::run(
        "cargo",
        &[
            "build",
            "--release",
            "--locked",
            "--bins",
            "--manifest-path",
            &manifest.to_string_lossy(),
            "--target-dir",
            &target.to_string_lossy(),
        ],
        root,
    )?;
    Ok(target.join("release"))
}

struct Compilers {
    c: String,
    cxx: String,
    prefix: Vec<String>,
}

impl Compilers {
    fn detect() -> Result<Self> {
        let prefix = report::output("brew", &["--prefix"])
            .map(|prefix| {
                vec![
                    format!("-I{prefix}/include"),
                    format!("-L{prefix}/lib"),
                    format!("-Wl,-rpath,{prefix}/lib"),
                ]
            })
            .unwrap_or_default();
        Ok(Self {
            c: std::env::var("CC").unwrap_or_else(|_| "cc".to_owned()),
            cxx: std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned()),
            prefix,
        })
    }

    fn c(&self, root: &Path, name: &'static str, stem: &str, libs: &[&str]) -> Result<Built> {
        let source = root.join(SOURCES).join(format!("{stem}.c"));
        let program = root.join(OUT).join(stem);
        let mut args = compile_args(&source, &program, "-std=c17");
        args.extend(self.prefix.iter().cloned());
        args.extend(libs.iter().map(|lib| (*lib).to_owned()));
        compile(&self.c, &args, root, name)?;
        Ok(Built {
            name,
            version: None,
            program,
        })
    }

    fn gnuradio(&self, root: &Path) -> Result<Built> {
        let flags = report::output(
            "pkg-config",
            &[&["--cflags", "--libs"], GNURADIO_MODULES].concat(),
        )
        .context("find GNU Radio with pkg-config (brew install gnuradio pkgconf)")?;
        let source = root.join(SOURCES).join("gnuradio.cc");
        let program = root.join(OUT).join("gnuradio");
        let mut args = compile_args(&source, &program, "-std=c++17");
        args.extend(flags.split_whitespace().map(str::to_owned));
        compile(&self.cxx, &args, root, "GNU Radio")?;
        Ok(Built {
            name: "GNU Radio",
            version: None,
            program,
        })
    }
}

fn compile_args(source: &Path, program: &Path, standard: &str) -> Vec<String> {
    vec![
        "-O3".to_owned(),
        native_flag().to_owned(),
        standard.to_owned(),
        source.to_string_lossy().into_owned(),
        "-o".to_owned(),
        program.to_string_lossy().into_owned(),
    ]
}

fn compile(compiler: &str, args: &[String], root: &Path, name: &str) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::run(compiler, &args, root).with_context(|| format!("build the {name} harness"))
}

fn native_flag() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "-mcpu=native"
    } else {
        "-march=native"
    }
}

pub fn lock_version(lock: &str, name: &str) -> Option<String> {
    let wanted = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    lines.by_ref().find(|line| line.trim() == wanted)?;
    lines
        .next()?
        .trim()
        .strip_prefix("version = \"")?
        .strip_suffix('"')
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_locked_version() {
        let lock = "[[package]]\nname = \"futuredsp\"\nversion = \"0.8.0\"\n\n[[package]]\nname = \"num\"\nversion = \"1\"\n";
        assert_eq!(lock_version(lock, "futuredsp").as_deref(), Some("0.8.0"));
        assert_eq!(lock_version(lock, "futuresdr"), None);
    }

    #[test]
    fn the_harness_lock_pins_futuredsp() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root")
            .to_path_buf();
        let lock = std::fs::read_to_string(root.join(SOURCES).join("rust/Cargo.lock"))
            .expect("the harness lock file");
        assert!(lock_version(&lock, "futuredsp").is_some());
    }

    #[test]
    fn timing_is_passed_as_three_arguments() {
        let timing = Timing {
            block: 8192,
            reps: 51,
            rep_seconds: 0.02,
        };
        assert_eq!(timing.args(), ["8192", "51", "0.02"]);
    }
}
