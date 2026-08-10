# PLAN-CANVAS.md — the canvas client (M7)

`PLAN.md` governs the project; this document governs the client rebuild decided in its §18
("Canvas-first client"). Same contract: binding until changed, changed only in writing, in the
same change as the code that deviates. Section numbers are stable — cite as `CANVAS §N`.

> **Status: M7 shipped.** Every phase in §8 is done and ticked there; what the build deviated
> from, and why, is recorded in the section it deviated from. `PROGRESS.md` records what was
> built and how it was verified.

The one-sentence idea: **the station is a patch, and the patch is the UI.** Every radio,
demodulator, decoder, map and file sink is a node on one canvas; wires carry typed streams
between them; a pin-board **rack** holds the faces being operated. The nearest relatives are
VCV Rack and Bespoke Synth, not GNU Radio Companion — GRC edits a program it then compiles
and runs, while this canvas *is* the running station: every wire is live, every change
applies immediately, and only graphs the engine can actually run are expressible.

Why: with more than one receiver, identity must be spatial. A tabbed UI answers "which SDR is
this?" with a dropdown; a canvas answers it with a labelled box the operator placed and the
wires leaving it.

---

## 1. The model — nodes, ports, faces

A **node** is ports + a **face**. The face is the node's instrument UI, rendered inside the
node and resizable; it is the primary control surface — there is no separate settings dialog
hiding the real knobs.

### Node catalog (initial)

| node | in | out | face |
|---|---|---|---|
| **Device** (RTL-SDR, HackRF, Soapy, file player, siggen) | — | `iq` | the tuning dial (signature element), gain stages, rate, PPM, bias-T — rendered from `Capabilities` as today |
| **Channel** (NFM/AM/SSB/WFM, ADS-B, AIS, POCSAG, RTTY, Morse, NAVTEX, ACARS, sub-GHz, …) | `iq` | `audio`? · `events`? | offset dial (position in the passband) + mode settings (squelch, bandwidth, AGC…) + its decoded output |
| **Scope** (spectrum + waterfall) | `iq` | — | the WebGL plot. One component, patched anywhere; on a device it is the band view |
| **Map** | `events` ×N | — | MapLibre; layers per connected decoder |
| **Decoder log** | `events` ×N | — | table, filters |
| **Speaker** | `audio` ×N | — | volume, mute, per-input mix (mixing is client-side, `PLAN §9`) |
| **Recorder** (SigMF) | `iq` | — | record control, disk meter |
| **Export** (CSV/JSON) | `events` ×N | — | filter + download (fronts the decoder-log export API) |
| **Scanner** | `iq` | — | range editor + live sweep; the edge *is* the tuning ownership (§9) |

**Built shorter than this table, on purpose.** Three node kinds are missing from the shipped
catalog because their *backends* are unbuilt: the **GPS source** (PLAN §13 Phase 4), the **UDP
sink** and the **WAV audio-file sink** (both Phase 2). A node whose backend does not exist is a
face that can only apologise, and a port whose type nothing emits is a wire that can only
dangle. Each lands with its feature, as one entry in `patch.rs` and one face. The **Scanner** row
above is the answer to §9, added when its face was ported.

### Port types

Hue encodes data type — and *only* data type. Every colour is paired with a marker shape and
a label, so no state rides on hue alone (colorblind operators read the graph by shape). The
values are role tokens in `web/src/index.css` (`--color-port-*`), measured in `DESIGN.md`.

| type | carries | marker |
|---|---|---|
| `iq` | wideband complex baseband at device rate | filled circle |
| `audio` | 48 kHz demod audio (Opus on the wire) | diamond |
| `events` | typed decoder frames (`DecodedRecord`) | square |

`iq-tap` (decimated channel IQ) and `position` (GPS) are **not defined**, for the same reason
their nodes are not: the channel analyzer and the GPS source are unbuilt (PLAN §13 Phase 2/4),
and this enum is what the port table is validated against. They arrive with their features.

### Connection rules (enforced at drag time, and again by the server)

- `iq` fans out: one device feeds N channels + scopes + a recorder — that *is* today's
  device set, drawn instead of implied. Outputs always fan out; only inputs constrain arity.
- A channel has exactly **one** `iq` input. Two devices into one channel is refused until
  `CoherentArray` exists (`PLAN §6`).
- Rate rules surface on the wire: ADS-B patched to a 2.4 Msps device shows the `PLAN §18`
  refusal *on the edge*, naming the rate that would work — a visible wire error, not a
  buried log line. The client does not re-derive the rule: `ChannelDescriptor.exact_rate_only`
  is computed by `channels` from the same two functions the engine's admission check uses, so
  the refusal the canvas predicts and the one the engine would give cannot disagree.
- `events` and `position` fan in freely on features (map, log, export).
- No cycle is expressible: only device nodes emit `iq`, only channels transform (`iq` in,
  reduced streams out), and everything else is terminal — edges can only flow
  source → channel → display/sink, so the port kinds themselves rule out a loop.

An invalid wire is refused with the reason where the operator is looking. This is the point
where we beat GRC rather than copy it: GRC lets you build graphs that fail at runtime; this
canvas can only express stations the engine can run.

---

## 2. The graph is control plane only

`PatchGraph` is a *description* the server validates and diffs into the existing engine
command queue — open device, add channel, start recorder, subscribe stream. A wire is never a
data path in itself; it names which existing stream (`PLAN §5`) a node consumes. The DSP
plane (`PLAN §7`) is untouched: no locks, no allocation, no graph scheduler on the hot path,
and the Pi 4 budget does not move.

---

## 3. Device identity (phase ①, prerequisite for everything)

- `PatchGraph` names devices as `DeviceRef { backend, serial }` plus a user label. Engine and
  probe ids stay per-run, as they are today.
- A referenced device that is not present renders as a **disconnected node**: controls
  disabled, wires kept, state preserved — never silently rebound to another radio.
- Serial-less duplicate clones bind at most one node; `--doctor` suggests programming an
  EEPROM serial. A serial-less singleton is fine — `{backend, serial: none}` is unambiguous.
- **Built with a `key` tie-break this section did not name.** A backend can have several devices
  and no serials: the virtual backend's key is `siggen` or the stem of a recording, both durable,
  and without it a patch could not say *which* capture it plays. `key` is consulted only when
  there is no serial, which keeps it away from the case it would be wrong for — an RTL-SDR clone
  whose key is a bus index. Resolution order: serial, then key, then a backend with one device.
- **Bindings are computed, never stored.** A device node claims the first unclaimed set or
  attached radio it matches, in stored node order; a channel node binds the n-th engine channel
  of its type on that set. The same two rules run server-side in `apply_station` and client-side
  in `web/src/canvas/binding.ts`, each with tests, because the face the canvas draws must be the
  channel the server's apply created.

This also retires the M6 "panels name no device set" deferral for good.

---

## 4. Wire model & persistence

- `PatchGraph { nodes: [{ id, kind, data?, position, size?, label? }], edges: [{ from: (node,
  port), to: (node, port) }] }` and `RackLayout { slots: [{ node, x, y, w, h }] }` live in
  `crates/wire` (serde + utoipa → generated TS), our model — **never React Flow's
  serialization** (same rule and reasons as the M6 layout tree, `PLAN §18`).
- **No `pinned` field**, unlike the sketch above: rack membership is the single truth for "this
  face is being operated", and two representations of one fact drift.
- Node settings stay where they live today (channel settings structs, device settings); the
  graph stores topology and geometry, not a second copy of settings. A channel node names its
  **type** only — that is topology, since the type decides the node's ports — so turning a
  squelch knob is not a workspace write, and two clients editing different channels cannot 409
  each other over one snapshot blob.
- **Applying a patch is additive and idempotent** (`POST /api/workspaces/{id}/apply`): it opens
  the radios the graph names and creates the channels it draws, and never closes or deletes
  anything. Removing a node is its own gesture with its own endpoint; a reconciler that also
  deleted would turn "this workspace has fewer nodes than the engine has channels" — the normal
  state when a second client adds one — into "close that operator's radio". Because it is
  idempotent it runs on every station load, which is what makes a restart come back as a
  station rather than an empty canvas. What it cannot satisfy (an absent radio, a wideband
  channel at the wrong rate) is *reported*, never silently skipped.
- One snapshot blob per workspace row, written atomically, revision-checked: a stale write is
  a 409 → refetch → re-apply, exactly the M6 concurrency rule. One workspace active at a
  time, unchanged.

---

## 5. The rack — the operate view

**Decision: a snapping grid, not a second canvas.** Shipped as a 24×24 grid; a face is dragged
by the grip over its header and resized from its bottom-right corner, both in whole cells, and a
placement that would overlap or leave the grid is refused rather than clamped. Operating wants alignment, density and
muscle memory — zero pan/zoom, no wires, faces on a fixed grid, dragged and resized by whole
cells. A second free canvas would just be the patch view with its wires hidden, and would
drift back into being one; if the rack ever feels cramped, the answer is bigger cells, not a
camera.

- Pin a node's face from the canvas; unpin returns it. The rack may be empty — the canvas
  alone is a complete UI.
- **One live surface per node:** a pinned face renders in the rack and the canvas node
  collapses to a compact "pinned →" placeholder. Never two GL contexts for one instrument.
- Keyboard-first carries over: tune step, mode, squelch, focus-next-face all bound.

---

## 6. Design direction

`DESIGN.md` is rewritten to this direction in phase ②; until then its physics rules (contrast
floors, plot-ink and colormap rules, tabular numerals, zero idle motion) still bind.

- **Bench at night:** near-black canvas ground with a faint dot grid; node chrome in quiet
  neutral graphite; colour lives on ports, wires, status and a thin category strip on the
  node header (source / channel / display / feature / sink).
- **Hue = data type** (§1 table); wires brighten on hover/selection; the accent's old jobs
  (selected, focused, tuned) live on.
- Kept from the old book because it is function, not paint: perceptually-uniform waterfall
  colormaps only, mono + `tabular-nums` for every changing number, no state by hue alone,
  nothing moves that the operator did not move.
- The dial remains the signature element — it is the face of every device node.

---

## 7. Tech

- **React Flow** (`@xyflow/react` 12, MIT). Custom nodes are ordinary React components →
  **Base UI parts throughout**, styled with our tokens; `isValidConnection` implements §1's
  rules against the generated types.
- **WebGL budget:** browsers cap live GL contexts (~8–16). One shared renderer draws every
  visible scope face (one context, many viewports), faces render only while on screen, and —
  because React Flow zooms with CSS transforms — plots re-render at zoom-adjusted device
  pixel ratio so they stay crisp instead of scaling as blurry bitmaps.
- **Desktop-only** (`PLAN §18`): pointer + keyboard assumed, laptop-class viewport minimum.

---

## 8. Migration — phases, each green before the next

Tests are part of every phase (`CLAUDE.md`), not listed per line: wire types get codegen +
drift checks, server handlers get handler tests + OpenAPI snapshots, validation gets unit
tests, and the canvas gets the Playwright smoke flow the web suite still owes.

1. ✅ **Identity + wire model.** `DeviceRef`, `PatchGraph`, `RackLayout` in `wire`; server
   endpoints + graph validation; codegen.
2. ✅ **Canvas shell.** React Flow canvas with device, scope, channel and speaker nodes, live
   wiring against the running engine. `DESIGN.md` rewritten here.
3. ✅ **Faces.** Every decoder panel becomes a node face; map, decoder log and the sink nodes
   land. (GPS did not: its backend is unbuilt — §1.)
4. ✅ **Rack.** Pin/unpin + grid; templates author patches (device + channels + wiring).
5. ✅ **Deletion.** dockview, tabs and the `LayoutNode` tree removed from `wire`, server and
   web. Stored M6 workspaces do not migrate — personal project, a clean reset is accepted
   and recorded here rather than buying a converter for layouts the new model cannot express.

**The transitional dual shell was skipped.** Phase ② said the dockview shell would ship
alongside the canvas until ⑤; all five phases landed in one change instead, so the two never
coexisted. Shipping both would have meant a second `PanelKind`-shaped wire model living beside
`PatchGraph` for the length of the change and being deleted unused — cost with no reader.

---

## 9. Open questions

- ~~**Scanner as a node**~~ — **closed: it is a node, wired to the receiver it drives.** The
  edge *is* the ownership, which is the only way to see at a glance which radio a running sweep
  has taken over; the face says in words that client retunes on that radio are refused while it
  runs (`PLAN §18`).
- ~~**Auto-placement**~~ — **closed by feel, as phase ② said it would be:** a node added from
  the palette lands to the right of everything already drawn, stepped down so it never covers
  what it was added next to. A channel does not spawn docked to a device: wiring it is the
  gesture that says which radio it is on, and pre-wiring would guess.
- **Subgraph macros** — a saved patch fragment ("airband bundle") placeable as one node;
  natural extension of templates, not designed yet.
- **Nodes for the library** — presets, bookmarks, templates and recordings ship in a drawer on
  the station bar, because they configure the radios that nodes name rather than carrying a
  stream. If one ever grows a stream (a preset that *is* a patch fragment, say), it becomes a
  node and leaves the drawer.
