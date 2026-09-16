import argparse
import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import urllib.request


VERSION = "9.0.1"
SHA256 = "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
ROOT = Path(__file__).resolve().parents[1]


def target_name():
    machine = platform.machine().lower()
    arch = "aarch64" if machine in ("arm64", "aarch64") else "x86_64"
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu", "Windows": "pc-windows-msvc"}[platform.system()]
    return f"{arch}-{suffix}"


# Both stay out of `target/`: Swatinem/rust-cache prunes that directory on a partial cache restore
# and would delete the installed libraries between the build and the link that needs them.
def install_prefix(target):
    return ROOT / ".media" / target


def work_dir(target):
    return ROOT / ".media-build" / target


def shell_env():
    env = os.environ.copy()
    extra = env.get("MEDIA_SHELL_BIN")
    if extra:
        env["PATH"] = os.pathsep.join([extra, env.get("PATH", "")])
    return env


# Windows `CreateProcess` resolves a bare name against the parent's `PATH`, not the one handed to
# the child, so every tool is looked up in the shell environment and spawned by absolute path.
def tool(name, env):
    found = shutil.which(name, path=env.get("PATH"))
    if found is None:
        raise RuntimeError(f"{name} is required to build FFmpeg")
    return found


def run(args, cwd, env):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def prepare_source(work, archive):
    if archive is None:
        archive = work / f"ffmpeg-{VERSION}.tar.xz"
        if not archive.exists():
            with urllib.request.urlopen(f"https://ffmpeg.org/releases/ffmpeg-{VERSION}.tar.xz") as response:
                archive.write_bytes(response.read())
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise RuntimeError("FFmpeg source checksum mismatch")
    source = work / f"ffmpeg-{VERSION}"
    if not source.exists():
        with tarfile.open(archive) as source_archive:
            source_archive.extractall(work, filter="data")
    return source


def configure(source, prefix, target, env):
    args = [
        tool("bash", env), str(source / "configure"), f"--prefix={prefix.as_posix()}",
        "--disable-autodetect", "--disable-everything", "--disable-network",
        "--disable-programs", "--disable-doc", "--disable-debug", "--disable-shared",
        "--enable-static", "--enable-pic", "--enable-gpl", "--enable-version3", "--disable-avdevice", "--disable-avfilter",
        "--enable-avcodec", "--enable-avformat", "--enable-swresample", "--enable-swscale",
        "--enable-decoder=aac,aac_latm,ac3,eac3,mp2,mpeg2video,h264,hevc",
        "--enable-parser=aac,aac_latm,ac3,mpegaudio,mpegvideo,h264,hevc",
    ]
    arch = target.split("-", 1)[0]
    args.append(f"--arch={arch}")
    if not shutil.which("nasm", path=env.get("PATH")):
        args.append("--disable-x86asm")
    if "windows-msvc" in target:
        # `lib.exe` rather than `llvm-lib`: configure picks an archiver's flags by asking it who it
        # is, and only the Microsoft banner selects `-out:`. llvm-lib answers with nothing configure
        # recognises, so it falls back to `ar rc` syntax and llvm-lib reads `rc` as a missing input.
        args.extend(["--toolchain=msvc", "--target-os=win32", "--cc=clang-cl", "--ld=lld-link"])
        # FFmpeg assembles its aarch64 kernels with armasm64 behind gas-preprocessor.pl, and the
        # Windows ARM64 runner carries neither.
        if arch == "aarch64":
            args.append("--disable-asm")
    elif "apple-darwin" in target:
        args.extend(["--target-os=darwin", f"--cc=clang -arch {'arm64' if arch == 'aarch64' else arch}"])
        if target != target_name():
            args.append("--enable-cross-compile")
    elif target != target_name():
        args.extend(["--enable-cross-compile", "--target-os=linux", f"--cross-prefix={arch}-linux-gnu-"])
    return args


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", default=target_name())
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--print-prefix", action="store_true")
    args = parser.parse_args()
    prefix = (args.prefix or install_prefix(args.target)).resolve()
    if args.print_prefix:
        print(prefix)
        return
    fingerprint = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    marker = prefix / "sdrmm-build.txt"
    if marker.exists() and marker.read_text() == fingerprint:
        print(prefix)
        return
    work = work_dir(args.target)
    work.mkdir(parents=True, exist_ok=True)
    env = shell_env()
    source = prepare_source(work, args.archive)
    run(configure(source, prefix, args.target, env), work, env)
    run([tool("make", env), "-j", str(os.cpu_count() or 2)], work, env)
    run([tool("make", env), "install"], work, env)
    marker.write_text(fingerprint)
    print(prefix)


if __name__ == "__main__":
    main()
