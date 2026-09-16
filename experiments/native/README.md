# sdr-- native (experiment)

The sdr-- interface drawn as a native window with
[zgui](https://github.com/zortax/zgui) instead of React in a webview.

It is a separate workspace on purpose. zgui is pre-1.0, pulls in around sixty crates, and is
consumed from Git, so the main workspace excludes `experiments/` and neither CI nor
`cargo xtask check` builds any of this.

## Running it

```sh
cd experiments/native
cargo run
```

The binary starts the real engine and the real `sdrmm-server` router on `127.0.0.1:0`, exactly as
the Tauri shell does, and talks to it over REST and the WebSocket with the `sdrmm-wire` types. To
drive a server that is already running instead:

```sh
cargo run -- --server http://localhost:8080
```

An untouched workspace is seeded with a starter patch — signal generator, scope, NFM channel,
speaker, decoder log — centred on 100 MHz with the channel on 100.3 MHz.

## What works

- The patch canvas: nodes positioned from the workspace graph, bezier wires coloured by port type,
  pan, node dragging, removal, and port-to-port wiring by clicking an output then an input.
- Node faces for the device (dial, radio, sample rate, DC block, overruns, forget), channels
  (dial, level bar, squelch, AGC, blanker, de-click, denoise, auto notch, passband), the scope,
  the speaker, and the decoder log.
- The frequency dial: click a digit, then the wheel or the arrow keys move that decade.
- The scope: spectrum trace, dB and frequency axes and channel markers on a `canvas`, and the
  waterfall on an embedded wgpu `surface` — a scrolling `R8Unorm` history texture with a palette
  lookup in WGSL, which is the same shape as the WebGL waterfall in `web/src/gl`.
- Live state over the WebSocket: device sets, channel levels, decoded records, spectrum frames.
- Searchable node palette with category filters and constructors for the entire server catalog.
- Decoder settings with typed toggles, choices, text and numeric fields generated from the wire
  schema, descriptor limits, and contextual NFM tone and scrambler controls.
- The rack view and ordered workspace undo/redo. Graph edits preserve rack placement and workspace
  settings; graph, radio and channel writes are serialized. Revision conflicts surface to the user.

## What is missing

- No audio output. The speaker face shows what is wired to it and its level; it does not decode
  Opus or open an output device.
- No map, video, images, scanner, hunt, coherent or CPS panels.
- Several specialized node faces still show placeholders, although they can be added and wired.

## Layout

| Path | Purpose |
|---|---|
| `src/host.rs` | The embedded engine and server, or a remote base URL |
| `src/api.rs` | REST client over `sdrmm-wire` types |
| `src/socket.rs` | WebSocket client, commands out, events and spectrum frames in |
| `src/store.rs` | The reactive state everything reads |
| `src/binding.rs` | Which device set and channel each graph node is bound to |
| `src/starter.rs` | The patch an untouched workspace is seeded with |
| `src/ui/patch.rs` | The canvas: wires, dot grid, node cards, ports |
| `src/ui/faces.rs` | What each node kind shows |
| `src/ui/gpu.rs` | The waterfall's wgpu pipeline |
| `src/ui/scope.rs` | The spectrum trace, axes, and palettes |
| `src/ui/widgets.rs` | Dial, select, segmented control, slider, checkbox, meters |
| `src/params.rs` | Decoder control metadata and validation from the shared wire schema |
| `src/ui/params.rs` | Decoder settings and native text entry |
| `src/workspace.rs` | Ordered workspace and settings writes |

`cargo test` covers the parts that are not drawing: frame decoding, node binding, the dial's
arithmetic, port geometry, wire curves, palette lookups, and the starter patch. Server-backed
smoke tests cover ordered saves and undo/redo, preservation of rack/settings, external revision
conflicts, and decoder edits against a registry containing only `device-virtual`.

```sh
cargo fmt --check
cargo clippy --no-default-features --all-targets -- -D warnings
cargo check
cargo test --no-default-features
```
