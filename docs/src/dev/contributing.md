# Contributing

`CLAUDE.md` in the repository root is the binding working agreement for every contributor,
human or otherwise. This page summarizes it; the file governs.

## The plan is the source of truth

`PLAN.md` decides architecture, crate boundaries, transport and scope. If a change needs to
deviate, **update `PLAN.md` in the same change and say why**. The plan is allowed to evolve;
silent drift from it is not.

Work milestone by milestone (`PLAN.md` §16). Do not build later-phase features before their
milestone unless asked.

## Non-negotiables

1. **One source of truth for wire types.** All DTOs, WebSocket messages and settings live once
   in `crates/wire`. TypeScript is generated. Never hand-write a TS type mirroring a Rust
   struct — regenerate. This blocks review.
2. **Tests are part of the work.** `dsp` primitives get golden-vector or analytic tests;
   decoders get a fixture plus expected output; the engine is tested end-to-end through
   `device-virtual`; the server gets handler tests, an OpenAPI snapshot and the codegen-drift
   check. A change that cannot be tested needs a written reason.
3. **No useless comments.** Comment *why*, never *what*: invariants, units, protocol quirks,
   references to a spec section. Do not narrate the code or restate a signature. Delete
   commented-out code.
4. **Fix problems where you find them.** If you touch a file and its pattern is wrong or
   stale, fix it. Do not copy a bad pattern forward. If the fix is too big for the change,
   note it — do not ignore it.
5. **Match the established pattern.** Read the neighbouring code first: naming, error
   handling, module layout, test style. Consistency beats preference.

## Code rules

- No `unwrap()` / `expect()` outside tests and startup — the workspace lints warn on them.
  Use `Result` and the project's error types.
- No silent failure. A dropped decoder frame, a truncated recording or an ignored device
  setting must surface as an error, a counter, or both.
- Hot DSP path: no locks, no allocation, no async. Settings arrive on a command queue, state
  leaves through snapshot channels.
- Keep functions small and single-purpose. Prefer clear names over comments.

## Toolchain

Always the newest stable versions and their current recommended patterns — check the current
docs before writing against a crate, do not code from memory.

| Area | Choice |
|---|---|
| Rust | The pinned nightly in `rust-toolchain.toml`, edition 2024, next-gen borrow checker. Bump the pin deliberately, with CI green. |
| Frontend | TypeScript 7 (`tsgo`), strict. React 19, TanStack Query, shadcn/ui on Base UI, Tailwind v4. |
| Formatting and linting | **Biome** (format + organize imports) and **Oxlint** with type-aware rules. **No ESLint, no Prettier** — do not add them, do not reintroduce their configs. |
| Package manager | pnpm for `web/`. |

Server state lives in TanStack Query only, invalidated by WebSocket events. Never poll.

## The gate

```sh
cargo xtask check   # fmt · clippy -D warnings · Soapy-free build · biome ci
                    # · oxlint --type-aware · tsgo · web build · codegen drift
cargo xtask test    # Rust + web suites, on device-virtual
```

Both must be green before every commit. CI runs the same two commands, so a local pass is a
real signal rather than a hint.

## Definition of done

- [ ] Follows `PLAN.md`, or updates it in the same change with a reason.
- [ ] `cargo fmt` and `cargo clippy -D warnings` clean.
- [ ] `biome ci`, type-aware `oxlint`, `tsgo` typecheck and the web build clean.
- [ ] Tests added or updated and passing; OpenAPI regenerated, no drift.
- [ ] No hand-written TS DTOs, no useless comments, no leftover dead code.
- [ ] Nearby off-pattern code fixed, not propagated.
- [ ] Newest versions used; no ESLint or Prettier introduced.

## Working on the docs

This site is [mdBook](https://rust-lang.github.io/mdBook/). Sources are in `docs/src`, the
table of contents is `docs/src/SUMMARY.md`, and configuration is `docs/book.toml`.

```sh
mdbook serve docs --open    # live preview
mdbook build docs           # what CI does; output in docs/book
```

`create-missing` is off deliberately: a table-of-contents entry pointing at a file that does
not exist fails the build instead of silently creating a blank page. `docs/book` is build
output and is not committed.

Pushing to `main` builds the book and deploys it to GitHub Pages. Pull requests build it
without deploying, so a broken book is caught before it merges.

Document what exists. If a feature is planned or in flight, say so on the page rather than
describing it as though it shipped — `PROGRESS.md` is the record of what is actually built and
green.
