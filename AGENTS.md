## Non-negotiables
1. One source of truth for wire types.
2. Tests are part of the work.
3. Never ever write a comment, code should be self explanatory!!!
4. Fix problems directly if you find them, without asking
6. Prefer self-written pure Rust. Use compatible open-source code or translate C/C++ only when necessary.

## Coding structure
- Respect crate boundaries: `dsp` has no I/O and no internal deps; `modem` builds reusable
  modulation algorithms on `dsp`; `channels` depends on `dsp`, `modem`, and `wire`.
- Adding a decoder should touch: one module in `channels`, one settings struct in `wire`,
  optionally one React panel. If it needs more, reconsider the design.
- One job, one node. Never two nodes, or a node and a device kind, that do the same thing: a radio
  is picked by a Device node and by nothing else. An abstraction over devices is a node wired to
  Device nodes, never a second kind of device that opens radios of its own. 
- No hidden flows, a flow should always be a wire. No hidden checkmark. It should always be visble that there is a connection trough a wire. 
- Hot DSP path: no locks, no allocation, no async. Settings via command queue, state via
   snapshot channels. Keep the control plane and DSP plane separated.
- Errors: no `unwrap()`/`expect()` outside tests and startup. Use `Result` and the project's
  error types. No silent failure: a dropped decoder frame or truncated result must surface.
- Keep functions small and single-purpose. Prefer clear names over comments.
- Always use the newest stable versions of every tool and dependency, and their current
  recommended patterns. Check the latest docs before writing.
- Always Format, Lint, Check & Test at the End of Every Change. But only test what you changed, full test suite will run in the CI.
- Max 3000 lines per file, 200 lines per function. Split large files into modules, large functions into helpers.

## Product
- Beginner-friendly, expert-deep
- Desktop-only, plus the field-mode remote head
- Never long text on a node. A face carries controls and readouts; what a setting means belongs in a title attribute or the docs, not in a paragraph on the canvas.
- Always write text, commits, docs as short as possible. Always try to reduce words, keep useless text out of the ui. And always write short and coherend text. That on which you are currently working on is not the most important thing in the world, you don't need to write everywhere about it. And never include an em dash.
