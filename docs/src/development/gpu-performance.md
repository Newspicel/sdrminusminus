# GPU measurements

Measured on an Apple M4 Max, 40 GPU cores, 48 GB, using Metal and wgpu 30.0.1.
Release build, Rust nightly 2026-08-01. Eight warmups and 101 timed iterations per
case. Times include upload, execution, synchronization and readback; exclude setup
and sample acquisition. These are measurements from this Mac, not every Apple M chip.

| Work | CPU median | GPU median | Choice |
| --- | ---: | ---: | --- |
| Spectrum, 4,096 points | 15.6 µs | 201.6 µs | CPU |
| Spectrum, four × 4,096, batched | 61.9 µs | 178.4 µs | CPU |
| Spectrum, sixteen × 4,096, batched | 251.5 µs | 222.4 µs | CPU; insufficient margin |
| Spectrum, four × 65,536, batched | 1.291 ms | 0.519 ms | Useful for larger future displays |
| Filter bank, 13 bands, 8,192 samples | 79.5 µs | 177.3 µs | CPU |
| Filter bank, 13 bands, 32,768 samples | 395.4 µs | 222.6 µs | Exceeds the current maximum block size |
| Radar correlation, 400,000 samples, 41 Doppler bins | 153.9 ms | 24.4 ms | GPU |
| Complete radar processing, same interval | 171.4 ms | 41.2 ms | GPU correlation, CPU cancellation and detection |

The complete radar comparison includes the optimized CPU clutter canceller on both
sides. Separately, stationary cancellation at 16,384 samples and 32 taps fell from
4.30 ms to 0.66 ms by reusing delayed correlations. No GPU transfer is needed for
that improvement.

[Raw medians and p95 timings](data/gpu-m4-max.csv).

## Runtime

The existing `gpu-fft` feature enables GPU radar too. Rust owns the buffers and
worker; portable WGSL uses 32-bit floats. Metal was verified here. Vulkan shader
parity runs in CI through Lavapipe; production rejects software adapters.

Radar tiles its transforms within a 128 MiB scratch budget. Unsupported workloads
and unavailable GPUs use CPU. GPU errors are logged and the affected interval is
recomputed on CPU. The capture thread submits preallocated jobs without waiting.
Overload is counted and logged; retunes and gaps discard stale work. Completed
reports are polled even when no new samples arrive.

Spectrum sizes below 65,536 stay on CPU regardless of lane count. Larger sizes use
GPU only if a startup comparison measures at least a 20% speed advantage. The
current 4,096-point display stays on CPU. The experimental batched spectrum and
filter-bank shaders remain benchmarks, not production processing paths.

## Reproduce

Run with access to the physical GPU:

```sh
cargo test -p sdrmm-engine --no-default-features --features gpu-fft --release --lib -- --ignored --nocapture --test-threads=1 gpu::benchmarks coherent::radar::tests::benchmark_radar_pipeline
```

Hardware correctness and fallback:

```sh
cargo test -p sdrmm-engine --no-default-features --features gpu-fft --release --lib -- --include-ignored --nocapture --test-threads=1 spectrum::tests gpu::caf::tests coherent::radar::acceleration::tests
```

## Rendering audit

The RF waterfall and audio spectrogram share the WebGL2 renderer. On this M4 Max,
Chrome 153 uses ANGLE Metal. Comparing the same shader with SwiftShader CPU rendering:

| Waterfall, CSS pixels at 2× scale | Metal draw median | Software draw median |
| --- | ---: | ---: |
| 640 × 240 | 0.076 ms | 2.236 ms |
| 1280 × 720 | 0.246 ms | 12.242 ms |

Keep WebGL2. Previously every visible plot repainted at display refresh, even with
no input. Plots now repaint only after data, settings, size, visibility or context
changes. At 30 incoming rows/s and 60 display frames/s, redraws halve; idle redraws
stop. Layout measurements continue so canvas zoom and resize remain responsive.

The cached Canvas 2D candidate also sustained 60 fps on Metal. Its CPU row coloring
and draw submission took 0.1–0.2 ms for the tested views. This does not establish a
CPU-only win: Canvas 2D may use GPU compositing, and this candidate omits retuning
and recoloring existing history. It also interpolates colors instead of intensities.
It remains a benchmark, not a replacement renderer.

A MapLibre fixture with 10,000 points sustained about 60 fps with both Metal and
SwiftShader. This test does not establish a map acceleration gain or justify a
backend replacement. It excludes basemap tiles, labels and network work. MapLibre
already stops continuous rendering when idle; no change was needed.

Browser cases use 30 warmup frames, 180 measured frames and 61 idle frames, with
one, four or eight views. GPU timer queries measure shader execution separately
from JavaScript submission. They exclude canvas copying and composition; frame
intervals capture missed display deadlines. The Canvas 2D numbers measure
submission, not synchronous completion. These are browser tests, not packaged
Tauri end-to-end measurements. WebKit 26.5 also passed on the Apple GPU, with
zero idle waterfall draws and p95 frame intervals of 16 ms. WebKit did not expose
GPU timer queries. Its coarser JavaScript clock limits small timing comparisons.

Raw results: [before](data/rendering-m4-max-before.jsonl),
[after](data/rendering-m4-max-after.jsonl),
[WebKit](data/rendering-m4-max-webkit.jsonl).

From `web`, with Chrome installed:

```sh
node scripts/benchmark-gpu.mjs
pnpm exec playwright install webkit
node scripts/benchmark-gpu.mjs --webkit
```
