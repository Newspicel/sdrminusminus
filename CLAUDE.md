## Non-negotiables
1. **One source of truth for wire types.** All DTOs / WS messages / settings live once in
   `crates/wire` (serde + utoipa). TypeScript is generated from OpenAPI. **Never hand-write a
   TS type that mirrors a Rust struct** — regenerate instead. This is a review-blocking rule.
2. **Tests are part of the work, not after it.** No feature is "done" without tests:
   - `dsp` primitives: golden-vector / analytic unit tests.
   - Decoders: a recorded IQ fixture + expected decoded output.
   - Engine: end-to-end via `device-virtual` (no hardware in CI, ever).
   - Server: handler tests + OpenAPI snapshot + codegen-drift check.
   - Performance gates tests for DSP paths to avoid regressions.
   A change that can't be tested needs a written reason in the PR.
3. **No useless comments.** Comment *why*, never *what*. Don't narrate the code, don't leave
   "changed X" / "this function does Y" noise, don't restate the signature. A comment earns its
   place only by stating a constraint the code can't (invariants, units, protocol quirks, refs
   to a spec section). Delete commented-out code.
4. **Fix problems where you find them.** If you touch a file and the pattern is wrong, stale, or
   inconsistent with the rest of the codebase, fix it — don't copy the bad pattern forward and
   don't leave it. Leave code better than you found it. If a fix is too big for the current
   change, note it, don't ignore it.
5. **Match the established pattern.** Before writing, read neighboring code and follow its
   conventions (naming, error handling, module layout, test style). Consistency beats personal
   preference. If the whole codebase's pattern is off, fix the pattern, don't fork a new one.
6. **Self-written pure Rust first** (portable by construction), if there is not a good reason not to or public rust code we can copy. (Only use dependencies from large repos; above 100 stars on)

## Coding structure
- Respect crate boundaries (§3): `dsp` has no I/O and no internal deps; `channels` depends only
  on `dsp` + `wire`; device backends are feature-gated; `server` is a library.
- Adding a decoder should touch: one module in `channels`, one settings struct in `wire`,
  optionally one React panel. If it needs more, reconsider the design.
- Hot DSP path: no locks, no allocation, no async. Settings via command queue, state via
  snapshot channels. Keep the control plane and DSP plane separated as the plan describes.
- Errors: no `unwrap()`/`expect()` outside tests and startup. Use `Result` and the project's
  error types. No silent failure — a dropped decoder frame or truncated result must surface.
- Keep functions small and single-purpose. Prefer clear names over comments.

## Always use the newest versions & guidelines
- **Always use the newest stable versions** of every tool and dependency, and their current
  recommended patterns. Check the latest docs before writing against a crate/library — don't
  code from stale memory. When a newer major exists, prefer it unless it breaks a hard
  constraint (note the exception if so).
- Use the **pinned Rust nightly** (`rust-toolchain.toml`) with the next-gen borrow checker;
  don't work around the pin. Bump it deliberately, with CI green.
- **Frontend toolchain (fixed choices, newest versions):**
  - **TypeScript 7** (native `tsgo` compiler), strict.
  - **Biome** for formatting + import organizing.
  - **Oxlint** for linting, **with type-aware rules enabled** (via `tsgolint`).
  - **No ESLint, no Prettier.** Don't add them, don't reintroduce their configs.
  - React 19 + TanStack Query + shadcn/ui on Base UI. Server state lives in TanStack Query
    only; invalidate via WS events, never poll.
- **CI is GitHub Actions**, added incrementally as the project matures — a workflow lands at
  M0 and grows each milestone. Every gate below must be runnable locally via `xtask`
  first, then mirrored in the workflow. Keep local and CI in lockstep.

## Definition of done (every change)
- [ ] Rust: `cargo fmt` + `cargo clippy -D warnings` clean.
- [ ] Web: `biome ci` clean, `oxlint` (type-aware) clean, `tsgo` typecheck + web build clean.
- [ ] Tests added/updated and passing; OpenAPI codegen regenerated, no drift.
- [ ] No hand-written TS DTOs; no useless comments; no leftover dead code.
- [ ] Nearby off-pattern code fixed, not propagated.
- [ ] Newest versions used; no ESLint/Prettier introduced.
- [ ] Comment and Push directly to main.

## Software Features
- Beginner-friendly, expert-deep
- Desktop-only
- Follow DESIGN.md for Design Principles
- Licensing stance (MIT, personal project): GPL projects (SDRangel, SDR++, DSDcc…) are fair game **as reference** for algorithms, parameters, and behavior — **no direct code copying**

When in doubt, re-read `PLAN.md`. If the plan is silent, pick the option most consistent with
the existing codebase and note the decision.
