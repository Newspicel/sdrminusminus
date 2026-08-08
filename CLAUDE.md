# CLAUDE.md — working agreement for sdr--

This file is binding for any AI or human contributor. Read it before touching code.

## The plan is the source of truth
- **`PLAN.md` governs.** Architecture, crate boundaries, transport, milestones, and scope all
  come from it. Do not invent structure that contradicts the plan.
- If a change requires deviating from the plan, **update `PLAN.md` in the same change** and say
  why. The plan is allowed to evolve — silent drift from it is not.
- Work milestone by milestone (§16). Don't build later-phase features before their milestone
  unless explicitly asked.

## Non-negotiables
1. **One source of truth for wire types.** All DTOs / WS messages / settings live once in
   `crates/wire` (serde + utoipa). TypeScript is generated from OpenAPI. **Never hand-write a
   TS type that mirrors a Rust struct** — regenerate instead. This is a review-blocking rule.
2. **Tests are part of the work, not after it.** No feature is "done" without tests:
   - `dsp` primitives: golden-vector / analytic unit tests.
   - Decoders: a recorded IQ fixture + expected decoded output.
   - Engine: end-to-end via `device-virtual` (no hardware in CI, ever).
   - Server: handler tests + OpenAPI snapshot + codegen-drift check.
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
  M0 and grows each milestone. Every gate below must be runnable locally via `xtask`/`just`
  first, then mirrored in the workflow. Keep local and CI in lockstep.

## Definition of done (every change)
- [ ] Follows `PLAN.md` (or updates it in the same change with a reason).
- [ ] Rust: `cargo fmt` + `cargo clippy -D warnings` clean.
- [ ] Web: `biome ci` clean, `oxlint` (type-aware) clean, `tsgo` typecheck + web build clean.
- [ ] Tests added/updated and passing; OpenAPI codegen regenerated, no drift.
- [ ] No hand-written TS DTOs; no useless comments; no leftover dead code.
- [ ] Nearby off-pattern code fixed, not propagated.
- [ ] Newest versions used; no ESLint/Prettier introduced.

## Commands (via xtask / just — the only entry points)
- `cargo xtask dev` — server + Vite dev server (HMR).
- `cargo xtask codegen` — regenerate OpenAPI + TS client. Run after changing `crates/wire`.
- `cargo xtask test` — full test suite (uses `device-virtual`, no hardware).
- `cargo xtask check` — the full local gate = fmt + clippy + `biome ci` + `oxlint`
  (type-aware) + `tsgo` typecheck + codegen-drift. Must be green before every commit;
  CI runs the same steps.
- `cargo xtask dist` — release artifacts.

When in doubt, re-read `PLAN.md`. If the plan is silent, pick the option most consistent with
the existing codebase and note the decision.
