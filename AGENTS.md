## Non-negotiables
1. One source of truth for wire types. All DTOs / WS messages / settings live once in `crates/wire`.
2. Tests are part of the work, not after it.
   - `dsp` primitives: golden-vector / analytic unit tests.
   - Decoders: a recorded IQ fixture + expected decoded output.
   - Engine: end-to-end via `device-virtual` (no hardware in CI, ever).
   - Server: handler tests + OpenAPI snapshot + codegen-drift check.
   - Performance gates tests for DSP paths to avoid regressions.
   - Client: unit and smoke tests
3. No comments, delete comments. Unless there is no way to read that out of the Code. Never refernce PLAN/FEATURE/DESIGN
4. Fix problems directly if you find them, without asking
5. Match the established pattern, except if you think its stupid. 
6. Self-written pure Rust first we can use/copy open source code or translate from C/C++ if we have to.

## Coding structure
- Respect crate boundaries: `dsp` has no I/O and no internal deps; `channels` depends only
  on `dsp` + `wire`; device backends are feature-gated; `server` is a library.
- Adding a decoder should touch: one module in `channels`, one settings struct in `wire`,
  optionally one React panel. If it needs more, reconsider the design.
- Hot DSP path: no locks, no allocation, no async. Settings via command queue, state via
   snapshot channels. Keep the control plane and DSP plane separated.
- Errors: no `unwrap()`/`expect()` outside tests and startup. Use `Result` and the project's
  error types. No silent failure — a dropped decoder frame or truncated result must surface.
- Keep functions small and single-purpose. Prefer clear names over comments.
- Always use the newest stable versions of every tool and dependency, and their current
  recommended patterns. Check the latest docs before writing.
- Always Format, Lint, Check & Test at the End of Every Change. But only test what you changed, full test suite will run in the CI. 
- Max 3000 lines per file, 200 lines per function. Split large files into modules, large functions into helpers.

## Product
- Beginner-friendly, expert-deep
- Desktop-only
- Keep comments concise and limited to non-obvious constraints, invariants, and rationale.
