## Non-negotiables
1. One source of truth for wire types. All DTOs / WS messages / settings live once in `crates/wire`.
2. Tests are part of the work, not after it.
   - `dsp` primitives: golden-vector / analytic unit tests.
   - Decoders: a recorded IQ fixture + expected decoded output.
   - Engine: end-to-end via `device-virtual` (no hardware in CI, ever).
   - Server: handler tests + OpenAPI snapshot + codegen-drift check.
   - Performance gates tests for DSP paths to avoid regressions.
   - Client: unit and smoke tests
3. Remove all comments!!! Its only for non-obvious constraints allowed in extremly rare cases. And never reference any PLAN/FEATURE/DESIGN.
4. Fix problems directly if you find them, without asking
5. Match the established pattern unless correctness, security, maintainability, or explicit project constraints require a deviation.
6. Prefer self-written pure Rust. Use compatible open-source code or translate C/C++ only when necessary. Record required attribution and license notices.

## Coding structure
- Respect crate boundaries: `dsp` has no I/O and no internal deps; `modem` builds reusable
  modulation algorithms on `dsp`; `channels` depends on `dsp`, `modem`, and `wire`.
  Modem measurements and file I/O live in `modem-test-support`, used only by tests and
  developer tooling. Device backends are feature-gated; `server` is a library.
- Adding a decoder should touch: one module in `channels`, one settings struct in `wire`,
  optionally one React panel. If it needs more, reconsider the design.
- One job, one node. Never two nodes, or a node and a device kind, that do the same thing: a radio
  is picked by a Device node and by nothing else. An abstraction over devices is a node wired to
  Device nodes, never a second kind of device that opens radios of its own.
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
- Desktop-only, plus the field-mode remote head
- Never long text on a node. A face carries controls and readouts; what a setting means belongs in
  a title attribute or the docs, not in a paragraph on the canvas.
