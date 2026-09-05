# Contributing to sdr--

## Make a change

1. Follow the [build and test guide](https://newspicel.github.io/sdrminusminus/development/building.html).
2. Branch from the latest `main`.
3. Keep the change focused and add tests that demonstrate the behaviour.
4. Run the checks for the parts you changed.
5. Open a pull request describing the result and how you verified it.

For a substantial new feature, discuss the behaviour and crate boundaries in an issue first.

## Code boundaries

- Define shared REST, WebSocket, settings, and patch types in `crates/wire`. Generate OpenAPI and
  TypeScript declarations from them.
- Keep `crates/dsp` free of I/O and internal dependencies. Reusable modem algorithms belong in
  `crates/modem`; measurement and file tooling belongs in `crates/modem-test-support`.
- Keep locks, allocation, and async work out of the hot DSP path. Send settings through command
  queues and publish state through snapshot channels.
- Open radios through Device nodes and feature-gated backends. Nodes that combine devices use
  existing Device streams.
- Report overruns, dropped frames, truncated recordings, and other failures to the operator.
- Build frontend controls from server capabilities and descriptors.
- Prefer clear names and small functions. Reserve comments for rare, non-obvious constraints.
- Prefer Rust implementations. Preserve attribution and license notices for reused code or tables.

The [architecture guide](https://newspicel.github.io/sdrminusminus/development/architecture.html)
describes the crate dependencies and runtime data flow.

## Tests and checks

Use the narrowest test that demonstrates the change:

| Changed area | Required coverage |
|---|---|
| DSP primitives | Analytic or golden-vector tests, plus relevant performance gates |
| Decoders | IQ fixture and expected decoded output |
| Engine | End-to-end tests through `device-virtual` |
| Server | Handler tests, OpenAPI snapshot, and codegen drift |
| Client | Unit tests and affected browser flows |
| Documentation | `mdbook build docs`, local links, and heading anchors |

Automated tests must not require or enumerate real hardware. Use virtual devices and reviewed fixtures.
Format, lint, check, and test the changed code. The full code gates are:

```sh
cargo xtask check
cargo xtask test
```

Additional checks depend on the change:

| Command | Use for |
|---|---|
| `cargo xtask smoke` | Browser workflows |
| `cargo xtask desktop` | Tauri shell |
| `cargo xtask perf` | DSP allocation and throughput |
| `cargo xtask audit` | Dependency changes |

For manual hardware tests, record the receiver model, driver and module versions, operating system,
test duration, reconnect result, and overrun or underflow counts in the pull request.

## Generated files

Commit generated output with its source change:

- API or wire types: `cargo xtask codegen`.
- Dependencies: `cargo xtask licenses`.
- `web/pnpm-lock.yaml`: `cargo xtask nix-hash` to update the Nix pnpm store hash. This requires
  Nix on Linux or a `nixos/nix` container elsewhere.

The [generated-file reference](https://newspicel.github.io/sdrminusminus/development/building.html#generated-files)
also covers decoder fixtures, band plans, and icons. `cargo xtask check` detects stale generated
contracts and a changed pnpm lockfile whose recorded digest was not updated.

## Pull requests

Explain what changed, why it belongs in the chosen layer, and how you tested it. Include any
remaining limitations. Keep unrelated cleanup in a separate change.

Contributions are licensed under the repository's
[GNU General Public License, version 3 or later](LICENSE).
