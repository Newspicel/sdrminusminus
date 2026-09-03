# Contributing to sdr--

Thanks for helping improve sdr--. The project welcomes focused bug fixes, hardware support,
decoder work, interface improvements, tests, and documentation.

## Start here

1. Read the [build and test guide](https://newspicel.github.io/sdrminusminus/development/building.html).
2. Create a branch from the latest `main`.
3. Keep the change focused and add tests at the narrowest layer that proves the behavior.
4. Run the relevant local gates.
5. Open a pull request that explains the user-visible outcome, validation, and any hardware used.

For substantial new features, open an issue first so the behavior and crate boundary can be agreed
before implementation work grows around it.

## Design rules

- Put shared REST, WebSocket, settings, and patch types in `crates/wire`; regenerate the OpenAPI
  and TypeScript outputs instead of declaring parallel client types.
- Keep I/O out of `crates/dsp`. The real-time signal path should avoid locks, allocation, async
  work, and silent loss.
- Keep hardware behind `sdrmm-device` traits and a feature-gated backend.
- Surface faults and dropped data. An overrun, truncated recording, or missed decoder frame is
  behavior the operator needs to see.
- Prefer capabilities and descriptors supplied by the server over device-name or channel-name
  tables in the frontend.
- Do not require real hardware in automated tests. Use `sdrmm-device-virtual`, protocol test
  generators, and short reviewed IQ fixtures.

The [architecture guide](https://newspicel.github.io/sdrminusminus/development/architecture.html)
explains the boundaries and data flow in more detail.

## Validation

The normal code gates are:

```sh
cargo xtask check
cargo xtask test
```

Run additional checks when the affected surface needs them:

```sh
cargo xtask smoke      # browser flow
cargo xtask desktop    # Tauri shell
cargo xtask audit      # dependency advisories
mdbook build docs      # documentation site
```

Hardware changes should include the receiver model, driver and module versions, operating system,
test duration, reconnect result, and any overrun or underflow observations in the pull request.

## Generated output

Run `cargo xtask codegen` after API or wire-type changes and `cargo xtask licenses` after dependency
changes. A change to `web/pnpm-lock.yaml` also moves the hash the Nix package pins the fetched pnpm
store by: `cargo xtask nix-hash` retakes it, through nix on Linux or a `nixos/nix` container
anywhere else, and `cargo xtask check` refuses a lockfile that has moved past the recorded one. Decoder fixtures, band plans, and icons have their own `xtask` commands documented in the
[development guide](https://newspicel.github.io/sdrminusminus/development/building.html#generated-files).
Commit generated outputs with the source change.

## Pull requests

A useful pull request description answers three questions:

- What changes for an operator or contributor?
- Why is this the right layer and design?
- How was it verified?

Keep unrelated cleanup separate. Review is much easier when the diff, tests, and explanation all
describe one coherent outcome.

By contributing, you agree that your contribution is licensed under the repository's
[GNU General Public License, version 3 or later](LICENSE).
