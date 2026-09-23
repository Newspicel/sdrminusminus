# Contributing to SDR--

## Make a change

1. Set up with the [build guide](https://sdrmm.newspicel.dev/development/building.html).
2. Branch from the latest `main`.
3. Keep the change focused, with tests that show it works.
4. Format, lint, check, and test the parts you changed.
5. Open a pull request saying what changed and how you verified it.

Open an issue first for large changes to behaviour or crate boundaries.

## Rules

- Define REST, WebSocket, settings, and patch types once, in `crates/wire`.
- `crates/dsp` does no I/O and depends on no project crate. Reusable modem algorithms go in
  `crates/modem`.
- No locks, allocation, or async in the hot DSP path. Settings go in through command queues,
  state comes out through snapshots.
- Only Device nodes open radios. Nodes that combine radios use Device streams.
- Every flow is a visible wire. No hidden connections.
- Never fail silently. Report drops, truncated recordings, and other losses.
- Build frontend controls from what the server reports.
- No comments. Clear names and small functions instead.
- Prefer pure Rust. Keep attribution and licenses for anything reused.

Adding a decoder should touch one module in `channels`, one settings struct in `wire`, and
optionally one React panel. See [Architecture](https://sdrmm.newspicel.dev/development/architecture.html).

## Tests

Use the narrowest test that proves the change:

| Area | Test with |
|---|---|
| DSP | Analytic or golden vectors, plus `cargo xtask perf` |
| Decoders | A recorded IQ fixture and its expected output |
| Engine | End-to-end runs through `device-virtual` |
| Server | Handler tests, OpenAPI snapshot, codegen drift |
| Client | Unit tests and affected browser flows |
| Docs | `mdbook build docs`, links, and anchors |

Automated tests never use real hardware. The
[checks table](https://sdrmm.newspicel.dev/development/building.html#checks) lists every gate, and
[generated files](https://sdrmm.newspicel.dev/development/building.html#generated-files) lists what
to regenerate.

For manual hardware tests, report the radio, driver version, OS, duration, reconnect result, and
drop counts.

## Pull requests

Say what changed, why it belongs in that layer, how you tested it, and what is still missing.
Keep unrelated cleanup separate.

Contributions are licensed under the [GNU General Public License, version 3 or later](LICENSE).
