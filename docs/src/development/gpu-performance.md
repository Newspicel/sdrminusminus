# GPU measurements

Where the GPU pays off, and where it does not. Measured on one Apple M4 Max (40 GPU cores,
48 GB) with Metal and wgpu 30.0.1, release build, Rust nightly 2026-08-01. Each case ran 8 warmups
and 101 timed iterations. Times include upload, execution, and readback, but not setup. Other
chips will differ.

| Work | CPU median | GPU median | Runs on |
| --- | ---: | ---: | --- |
| Spectrum, 4,096 points | 15.6 µs | 201.6 µs | CPU |
| Spectrum, 4 × 4,096 | 61.9 µs | 178.4 µs | CPU |
| Spectrum, 16 × 4,096 | 251.5 µs | 222.4 µs | CPU, margin too small |
| Spectrum, 4 × 65,536 | 1.291 ms | 0.519 ms | GPU would win for larger displays |
| Filter bank, 13 bands, 8,192 samples | 79.5 µs | 177.3 µs | CPU |
| Filter bank, 13 bands, 32,768 samples | 395.4 µs | 222.6 µs | Larger than any real block |

[Hardware tests](hardware-tests.md#measured) has passive radar timings on a KrakenSDR.

[Raw medians and p95](data/gpu-m4-max.csv).

## How it runs

The `gpu-fft` feature turns on GPU spectra and radar. Shaders are portable WGSL with 32-bit
floats. Metal is verified on hardware; the CI `gpu` job checks Vulkan parity on Lavapipe. Software
adapters are refused in production.

The radar correlation uses at most 512 MiB of GPU memory. If it needs more, has no usable GPU, or
hits a GPU error, it runs on the CPU instead and logs a warning. The capture thread hands off
preallocated jobs without waiting. Overload is counted; retunes and gaps discard stale work.

Spectra below 65,536 points always stay on the CPU. Larger ones use the GPU only if a startup
benchmark shows it at least 20% faster. The batched spectrum and filter-bank shaders are
benchmarks only.

## Reproduce

With access to the physical GPU:

```sh
cargo test -p sdrmm-engine --no-default-features --features gpu-fft --release --lib -- --ignored --nocapture --test-threads=1 gpu::benchmarks
```

Correctness and CPU fallback on the hardware:

```sh
cargo test -p sdrmm-engine --no-default-features --features gpu-fft --release --lib -- --include-ignored --nocapture --test-threads=1 spectrum::tests gpu::caf::tests array::radar::gpu
```

## Rendering

The waterfall and audio spectrogram share one WebGL2 renderer. In Chrome 153 on the same M4 Max
(ANGLE Metal), compared with SwiftShader software rendering:

| Waterfall, CSS pixels at 2× | Metal draw | Software draw |
| --- | ---: | ---: |
| 640 × 240 | 0.076 ms | 2.236 ms |
| 1280 × 720 | 0.246 ms | 12.242 ms |

WebGL2 stays. Plots repaint only when data, settings, size, or visibility change: at 30 rows/s on
a 60 Hz display that is half the frames, and idle plots draw nothing.

Also tried, and not adopted:

- **Canvas 2D waterfall:** held 60 fps at 0.1 to 0.2 ms per frame, but skipped retuning and
  recolouring history and interpolated colours instead of intensities. Not a fair win.
- **MapLibre with 10,000 points:** about 60 fps on both Metal and SwiftShader. MapLibre already
  stops drawing when idle; nothing to change.

Each browser case ran 30 warmup, 180 measured, and 61 idle frames with 1, 4, or 8 views. GPU timer
queries measure shader time; frame intervals catch missed deadlines. WebKit 26.5 also passed with
zero idle waterfall draws and 16 ms p95 frames, but has no GPU timer queries. These are browser
numbers, not packaged Tauri measurements.

Raw results: [repaint every frame](data/rendering-m4-max-before.jsonl),
[repaint on change](data/rendering-m4-max-after.jsonl),
[WebKit](data/rendering-m4-max-webkit.jsonl).

From `web`, with Chrome installed:

```sh
node scripts/benchmark-gpu.mjs
pnpm exec playwright install webkit
node scripts/benchmark-gpu.mjs --webkit
```
