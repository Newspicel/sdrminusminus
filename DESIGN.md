# DESIGN.md — the visual and interaction system for sdr--

This file is binding the way `CLAUDE.md` is binding. `PLAN.md` §10 says *what* the client must
be and `PLAN-CANVAS.md` says what shape it takes; this says *how it looks and behaves*, in
numbers a reviewer can check. If a change needs a value that is not here, add it here in the
same change.

This is the canvas edition (`CANVAS §8` phase ②). It replaces the edition written for the
top-bar / tab-bar / dock shell. Two things are gone, not deprecated: the anodized single-accent
chrome as an organising idea — colour is now spent first on **what a wire carries** — and every
mobile and touch rule, because `PLAN §18` removed mobile support outright. What survived
survived because it is function, not paint: the contrast floors, the plot-ink discipline, mono
tabular numerals, the density scale, and "nothing moves that the operator did not move."

The reference points are the ones `PLAN` §10 names, now joined by the ones `CANVAS` names: bench
instruments, pro audio, and a modular rack — not landing pages, and not GRC. Everything below
follows from three commitments.

1. **The patch is the instrument.** The station is a graph the operator laid out, and the
   layout *is* the answer to "which radio is this?". Node chrome is the bezel around a face:
   quiet, dense, and out of the way.
2. **Colour is a budget spent on data.** A hue only appears where it carries meaning. Inside a
   plot rectangle the colormap spends the whole budget; outside it, the budget goes to the type
   a port and its wire carry, plus the four semantic roles. Nothing else gets any.
3. **Nothing moves that the operator did not move.** Steady state is zero motion. A live
   waterfall is data, not animation. A canvas never re-lays-out.

---

## 1. Direction

**Bench at night** (`CANVAS §6`). A near-black canvas ground with a faint dot grid; nodes in
quiet neutral graphite; colour on ports, wires, status, and a thin category strip on each node
header. The graphite is the same warm-neutral family as before, and for the same reason — it
separates chrome from the blue-violet that already lives in the magma colormap.

**Signature element: the dial.** Unchanged in role, moved in place: the frequency readout is a
machined tuning dial and it is the face of every device node (`CANVAS §6`). Each digit is its
own control — hover it, scroll it, arrow it, type over it. It is the only place in the UI with a
display-scale type size.

**Desktop-only.** `PLAN §18` removed mobile support and recorded the cost; `CANVAS §7` restates
it. The client assumes a pointer, a keyboard and a laptop-class viewport. Concretely, from the
previous edition of this file these rules are **deleted, not softened**: phone layouts, the
touch-first paths, viewport-guarded layout writes, the ≥40px coarse-pointer target floor, and
the coarse-pointer widening of spectrum marker hit areas. The floor that remains is §4's flat
24px, on every pointer. A canvas of wired boxes is not a phone UI and pretending otherwise
would cost density on the only viewport that exists.

> Debt this edition names rather than fixes: `web/src/components/controls.ts` still carries
> `pointer-coarse:min-h-10` / `pointer-coarse:size-10` variants. They are vestigial under the
> rule above and come out the next time that file is touched.

---

## 2. Colour

Authored in OKLCH, one hue family per role, with lightness carrying elevation (on dark surfaces
the shadow channel is algebraically bankrupt, so depth is surface lightness). The light theme is
the same role table re-anchored, never a hand-picked second set of hexes.

### Surfaces and ink

| role | dark | light | job |
|---|---|---|---|
| `bg` | `oklch(.185 .006 75)` | `oklch(.955 .004 80)` | app ground, canvas ground, elevation 0 |
| `panel` | `oklch(.225 .007 75)` | `oklch(.985 .003 80)` | node bodies, bars — elevation 1 |
| `panel-2` | `oklch(.265 .008 75)` | `oklch(.925 .005 80)` | node headers, control fills, wells — elevation 2 |
| `panel-3` | `oklch(.305 .009 75)` | `oklch(1 0 0)` | popovers, menus — elevation 3 |
| `line` | `oklch(.34 .008 75)` | `oklch(.865 .006 80)` | separators, node border at rest, the dot grid |
| `line-strong` | `oklch(.52 .009 75)` | `oklch(.60 .008 80)` | control borders, port marker rims |
| `ink` | `oklch(.92 .008 80)` | `oklch(.27 .012 75)` | primary text, values |
| `ink-dim` | `oklch(.70 .010 80)` | `oklch(.47 .012 75)` | labels, secondary text |
| `ink-faint` | `oklch(.60 .010 80)` | `oklch(.53 .012 75)` | micro-legends, units, node subtitles |

Elevation step is ΔL ≈ 0.04. Dark themes never use pure black (`bg` sits at L .185, inside the
.18–.24 halation-safe band) and never use pure white text (`ink` at L .92).

### Semantic

| role | dark | light | meaning |
|---|---|---|---|
| `accent` | `oklch(.80 .135 72)` | `oklch(.545 .125 62)` | interactive, selected, focused, tuned |
| `accent-dim` | `oklch(.62 .10 72)` | `oklch(.68 .10 62)` | accent at rest / underlays |
| `danger` | `oklch(.70 .16 27)` | `oklch(.51 .19 27)` | faults, destructive, rejected |
| `ok` | `oklch(.78 .14 155)` | `oklch(.50 .115 155)` | live, playing, recording-healthy |

Hue separation accent 72° / danger 27° / ok 155° holds in both themes, and **no state is carried
by hue alone** — every one pairs with a word, a glyph or a position.

**Measured contrast** (WCAG 2.1 ratio, sRGB, against `bg`):

| | ink | ink-dim | ink-faint | accent | danger | ok | line-strong |
|---|---|---|---|---|---|---|---|
| dark | 14.7 | 7.0 | 4.7 | 9.8 | 6.5 | 9.9 | 3.4 |
| light | 13.2 | 6.0 | 4.6 | 4.5 | 5.6 | 5.0 | 3.5 |

Every text role clears 4.5:1 in both themes; `line-strong` is a non-text boundary and clears
3:1. Dark carries the extra margin the polarity-blindness of the WCAG formula demands.

> The light `accent` above is `oklch(.545 .125 62)` = 4.51:1. **`web/src/index.css` still ships
> `oklch(.56 .125 62)` = 4.23:1**, which fails the text floor wherever the accent is text —
> `segment()` selected, `BTN` hover. The re-anchor is binding on the next change that touches
> that file. The previous edition of this document printed 4.2 and claimed the floor was clear;
> that claim was wrong and is not carried forward.

### Hue = data type

The rule, and it has no exceptions: **outside a plot rectangle, hue on a port, a wire or a
header strip encodes the type of thing that flows, and nothing else.** It never encodes state,
health, freshness, selection, ownership or severity — those are the four semantic roles above,
and each of them is paired with a word or a glyph. A hue that starts meaning two things has
stopped meaning either.

Every port therefore ships **hue + a marker shape + a text label**. The shape is what a
colourblind operator reads; the label is the accessible name and the hover title. With colour
removed entirely the graph must still be unambiguous — that is the acceptance test.

| type | carries | token (dark) | light (required) | marker | dark vs `bg` | light vs `bg` |
|---|---|---|---|---|---|---|
| `iq` | wideband complex baseband at the device rate | `oklch(.72 .11 235)` | `oklch(.592 .11 235)` | filled circle | 7.7:1 | 3.5:1 |
| `audio` | 48 kHz demodulated audio (Opus on the wire) | `oklch(.74 .12 158)` | `oklch(.583 .12 158)` | diamond (square rotated 45°) | 8.5:1 | 3.5:1 |
| `events` | typed decoder frames (`DecodedRecord`) | `oklch(.78 .11 85)` | `oklch(.599 .11 85)` | square | 9.3:1 | 3.5:1 |

Three types is the whole set the engine produces today and the set stays that small on purpose.
`CANVAS §1` reserves `iq-tap` and `position`; each arrives with a token, a distinct marker shape
and a measured row in this table, in the same change that adds it.

A wire and a marker are non-text graphics, so the floor is 3:1 against the ground they are drawn
on — the canvas `bg` for a wire, `panel` or `panel-2` for a marker. The dark column above is
measured against `bg`; against `panel-2` (the tightest ground a marker sits on) the same three
measure 6.3 / 7.0 / 7.6:1.

> **`index.css` does not yet re-anchor these for light.** Shipped, the dark values measure
> 2.1 / 1.9 / 1.8:1 against the light `bg` — below the 3:1 floor, and the light theme is
> currently unusable for reading a patch. The light column above is the fix and is binding on
> the next change that touches that file.

Port chroma is held ≤ .12 and category chroma ≤ .07, both under `accent`'s .135, so a lit wire
never outshouts a tuned control.

### Wires

- Drawn 1.5px in the hue of the port they leave.
- Hover and selection **brighten**: 2.5px and `brightness(1.35)`. They never recolour — the hue
  is already saying what the wire carries, and a wire that changed colour on hover would be
  hue carrying state.
- The in-flight connection line is 2px `accent`, because it is not carrying anything yet.
- A refused wire is never drawn. The refusal is stated in words where the operator is looking
  (`CANVAS §1`) — today a bottom-right toast naming the fix, e.g. the rate a decoder needs.

### Category strip

A 4px strip on the node header says what the box *is* before its label is read. Low chroma:
this is a silkscreen mark, not a status light. Measured against `panel-2`, the header ground:

| category | dark | light (required) | vs `panel-2`, dark |
|---|---|---|---|
| `source` | `oklch(.62 .07 235)` | `oklch(.594 .07 235)` | 4.3:1 |
| `channel` | `oklch(.62 .07 300)` | `oklch(.603 .07 300)` | 4.1:1 |
| `display` | `oklch(.62 .05 200)` | `oklch(.592 .05 200)` | 4.3:1 |
| `feature` | `oklch(.62 .07 60)` | `oklch(.602 .07 60)` | 4.1:1 |
| `sink` | `oklch(.58 .03 80)` | `oklch(.598 .03 80)` | 3.6:1 |

All clear the 3:1 non-text floor. The light column carries the same defect and the same
next-touch obligation as the port hues. The strip is never the only thing distinguishing two
nodes: the title, the ports and the face say it too.

### Plot ink — the rule that keeps the waterfall readable

**The plot never inverts and its overlays are achromatic.** The colormap owns hue inside the
plot rectangle, so anything drawn on top of it separates by *luminance and shape* only:
isoluminant edges are nearly invisible, and a coloured cursor over a colormap is a conjunction
search. Data-type hue stops at the plot's edge for the same reason. The plot has its own token
set, identical in both themes:

| token | value | job | vs `plot-bg` |
|---|---|---|---|
| `plot-bg` | `oklch(.16 .008 75)` | behind the trace and unwritten waterfall | — |
| `plot-grid` | `oklch(.55 .006 80)` @ 14% | gridlines, always lighter-weight than data | — |
| `plot-trace` | `oklch(.93 .020 85)` | the live spectrum line | 15.8:1 |
| `plot-hold` | `oklch(.62 .015 85)` | max-hold trace | 5.3:1 |
| `plot-ink` | `oklch(.97 .010 85)` | selected marker, axis text | 17.8:1 |
| `plot-ink-dim` | `oklch(.72 .010 85)` | unselected markers, tick labels | 7.8:1 |

The one licensed exception is the *label chip* on a marker: it sits on its own opaque `bg`
plate, outside the colormap, so it may carry `accent` to mark selection.

### Waterfall colormaps

All shipped colormaps are perceptually uniform and monotone in luminance: jet and its relatives
are excluded on purpose. `magma` is the default; `inferno`, `plasma`, `viridis` and `gray` are
selectable. The choice is a per-eye preference, so it lives in `localStorage`, not in the
workspace. The waterfall advances one history row per CSS pixel, so its scroll rate is the frame
rate and no arriving row is ever skipped.

### Theme

Three states: `system` (default), `dark`, `light`. Stored in `localStorage` — a theme is a
property of the eye looking at the screen, not of the station, so unlike workspaces it does not
sync between clients.

---

## 3. Type

No webfonts. The server ships as a self-contained binary for a Pi in a field (`PLAN` §15), so a
font download is a dependency the product cannot honour; personality comes from the scale and
the treatment instead of the file.

- **UI:** system sans stack.
- **Data:** system mono stack, `tabular-nums` + `slashed-zero`, everywhere a number can change
  or be compared. Live values that jitter horizontally are a layout-stability bug, not a
  cosmetic one. In practice: every number in a face carries `font-mono`.
- **Legends:** 10px uppercase, `0.09em` tracking, `ink-faint` — the silkscreen voice. Used for
  units, column heads, section labels and **every node title and subtitle**; never for values.

Scale (ratio ≈ 1.25, base 13px — dense operational surface):

```
10  legend      12  control/table      13  body (base)      16  panel heading
20  emphasis    26  dial (compact)     34  dial (full)
```

Line height: 1.5 body, 1.2 headings, fixed px for table rows. Numeric columns are right-aligned
with uniform precision per column and reserve width for their live maximum.

---

## 4. Space, separation, density

Spacing scale `2 4 8 12 16 24 32 48`. Every margin, padding and gap comes from it. The canvas
dot grid is on the same scale at 24.

**Proximity is a ratio.** Between-group spacing ≥ 2× within-group. A divider is only drawn where
that ratio cannot be afforded — which is the test for every border in the UI.

**Separation ladder**, use the least that does the job: spacing → tint (`panel-2` well) →
hairline (`line`) → shadow. Shadow means *elevation*, so it appears only on popovers, menus and
drag ghosts. On dark surfaces those also get `+ΔL` surface lightness (`panel-3`) and a 1px top
highlight (`inset 0 1px 0 oklch(1 0 0 / .06)`), because a drop shadow on an L .2 ground cannot
reach popover-grade edge contrast at any alpha. **A node casts no shadow** — a canvas full of
floating cards is the look this project is not.

**Density: compact.** This is a monitoring surface an operator watches for hours. Control height
28px, table row 30px, panel and face padding 12px, node header 26px.

**Hit floor: 24px, every pointer.** Compact buys space by trimming padding, never hit area. The
drawn mark and the hit area are separate objects, and where the two differ this document states
both: a spectrum marker draws 1px and hits ≥12px; a port draws 10px and must hit ≥24px; a node
resize handle draws 8px and must hit ≥24px. §12 records where the shipped hit areas are still
the drawn size.

Radii are concentric, not equal: popover 6px, control 3px, chip 3px, node 0, plot 0. A node is a
panel bolted to a rail, not a card.

---

## 5. The canvas

The patch view. React Flow draws it; the stored document is ours (`CANVAS §4`).

**Ground.** `bg`, edge to edge, with a dot grid: **gap 24px, dot 1px, `--color-line`** (1.6:1
against `bg`). Strong enough to give the plane a scale and a sense of motion while panning, too
weak to compete with a node border at 1px `line` on `panel`.

**What the grid must not do.** It never gains salience from the camera. At minimum zoom the dots
must not fuse into bands, moiré, or read as a tint — if the spacing ever gets there the grid
drops out rather than densifies. At maximum zoom a dot stays 1px and stays `line`: it never
brightens, never thickens, never becomes a line grid, and never becomes a snap target. The grid
takes no pointer events and is not a layout constraint — node positions are free floats, and the
grid is a texture, not a rule.

**Camera.** Zoom range **0.25–2**, hard clamped. Below 0.25 a face is unreadable and the canvas
becomes a minimap that is not one; above 2 a node is a poster. Scroll pans, ⌘/Ctrl+scroll zooms
about the cursor, a drag on the pane rubber-band-selects, and a click on the pane clears the
selection. Pointer and keyboard are assumed (`PLAN §18`); there is no pinch path.

**Nothing auto-arranges.** No force layout, no auto-routing that moves a node, no "tidy" button
that relocates what the operator placed. Geometry is written once at the end of a gesture, never
per frame (`CANVAS §4`). Position is identity here — moving a node behind the operator's back
would destroy the one thing a canvas buys over a dropdown.

---

## 6. The node

A node is chrome plus a face. `NodeShell` owns every number in this section; a face draws none
of it.

| part | value |
|---|---|
| header height | 26px, `panel-2`, 1px `line` bottom |
| category strip | 4px wide, full header height, flush left, `--color-cat-*` |
| title | node label, else the kind's default; legend voice, `ink-dim`, truncating, left after the strip |
| subtitle | one line: what the node is bound to, or why it is not; legend voice, `ink-faint`, right-aligned, truncating |
| header actions | face actions, then pin, then remove; icon buttons drawn 20px (hit floor §4, debt §12), `ink-faint`, remove hovers to `danger` |
| body | `panel`, 1px border: `line` at rest, `accent` when selected |
| port marker | 10px, 1px `line-strong` rim, hue + shape per §2; inputs left, outputs right |
| first port offset | 26px from the node top — clear of the header |
| port stride | 22px; the two sides stack independently |
| minimum size | 220 × 140 |
| default size | 320 × 220 |
| radius | 0; no shadow |

**Selection** is the border going `accent`, nothing else. No fill change, no glow, no scale — a
selected node must stay legible as the same node.

**Disconnected is a first-class state** (`CANVAS §3`). A node that names a radio which is not
attached, or a channel whose receiver is missing, renders at **opacity .6**, with every control
that would command hardware disabled, and its subtitle says which radio it wants. Its wires stay
drawn at full strength and its settings stay stored; it is **never silently rebound** to another
radio. Absence is a state of the station, not a fault: it takes no `danger` hue, no toast and no
badge. The dimming plus the subtitle is the whole report.

**A face that cannot say anything at 220 × 140 is too big a face.** Give it a summary state at
minimum size and its detail above that, or split it into two nodes.

---

## 7. The face

The face is the instrument, and it is the only control surface — there is no settings dialog
behind it (`CANVAS §1`). Signature: `XFace({ node }: { node: PatchNode })`, wrapped in
`NodeShell`, with live state read from the station context and never from props.

**Gesture ownership.** Inside the face rectangle the instrument owns the wheel and the drag: a
scope zooms about the cursor, a dial digit steps, a slider drags. The canvas pans and
rubber-bands only from the pane, and a node moves only by a drag on its header or on inert body
space. Any control inside a face that itself takes a pointer drag or a wheel must opt out of the
canvas gestures (`nodrag` / `nowheel`), or the operator will move the node while trying to set a
value. This is the rule that keeps §5's camera and §9's plot gestures from claiming the same
event.

**Scrolling.** `FaceBody` scrolls its content by default; `scroll={false}` when the content owns
its own size — a plot, a map, a canvas.

**One live surface per node** (`CANVAS §5`, `§7`). Browsers cap live GL contexts at roughly
8–16, so a pinned face renders in the rack and its canvas node collapses to a compact
"pinned →" placeholder. Never two GL contexts for one instrument. GL faces render only while on
screen, and because React Flow zooms with a CSS transform they re-render at a zoom-adjusted
device pixel ratio — a zoomed plot is redrawn crisp, never upscaled as a bitmap.

**Errors never take a row.** No banner appears above a face and pushes it down; failures are
toasts in the bottom-right, dismissible, auto-expiring for the transient kinds. A banner that
appears and disappears moves everything under it, and the operator did not move it.

---

## 8. The rack

The operate view: a pin-board of the faces currently being worked, and deliberately **not** a
second canvas (`CANVAS §5`). Operating wants alignment, density and muscle memory.

| property | value |
|---|---|
| grid | 24 columns × 24 rows, whole cells only |
| camera | none — no pan, no zoom |
| wires | none — the patch view owns topology |
| default slot | 12 × 8 cells (four fit) |
| minimum slot | 1 × 1 cell |
| drag / resize | snap to whole cells; a move that would overlap or leave the grid is refused in place, never pushed or reflowed |
| pin placement | first free cell, scanning left-to-right then down |
| full rack | the pin is a no-op — a full rack is a rack, not an error |
| deleted node | its slot is dropped |

Pinning collapses the canvas node to a placeholder (§7); unpinning returns the face to it. The
rack may be empty — the canvas alone is a complete UI, and the rack is never a required stop.
Nothing in the rack ever reflows because something else moved: whole-cell placement with
refusal-on-overlap is chosen over a packing algorithm precisely so that muscle memory holds.

---

## 9. The dial and the scope

Both are faces now; their gestures are unchanged and are the reason §7's ownership rule exists.

### The dial — the device node's face

`FrequencyDial` renders the tuned centre as `100.000 000 MHz`, grouped MHz / kHz / Hz. Leading
zeros are drawn in `ink-faint` so magnitude is readable before any digit is parsed.

| gesture | effect |
|---|---|
| wheel over a digit | ± one unit of that digit's decade |
| click a digit's upper / lower half | ± one unit of that decade, and focus it (the dial is one tab stop; ←/→ move between digits) |
| hover a digit | tints the half under the pointer in `accent/18` and takes an `n-resize` / `s-resize` cursor — the direction is shown before the press, never after |
| ↑ / ↓ | ± one unit of the focused digit |
| PageUp / PageDown | ± ten units |
| 0–9 typed | write that digit and advance right |
| Enter or `f` | open direct entry — type `145.5`, `145m5`, `433800k` |
| Esc | cancel direct entry, restore the tuned value |

The active digit carries a 2px `accent` underline; the whole dial carries the focus ring when
focused. Every step clamps to the device's `frequency_range`, and a clamped step is silent — the
value simply stops, which is the honest report that the radio cannot go further. All of it is
arithmetic on integer Hz in `dial.ts`, unit-tested; the component only routes events.

### The scope — one component, patched anywhere

On a device's `iq` it is the band view; on a channel's tap it is the channel analyzer
(`CANVAS §1`). Trace on top, waterfall below, a draggable 1px divider between them (12px hit
area). The split fraction is client state. Frequency axis on the divider, dB scale in the
trace's left gutter, both drawn from the frame's own metadata.

**View transform.** Pan and zoom are a client-side window `[start, end] ⊆ [0,1]` over the device
span — the server streams a fixed span, so zooming magnifies rather than resolves, and the
readout says so by showing the *visible* span. Wheel zoom is the fixed-point affine transform:
the frequency under the cursor stays under the cursor. Pure math in `spectrumView.ts`,
unit-tested.

| gesture | effect |
|---|---|
| wheel | zoom about the cursor (1.2× per notch, clamped to 512× and to full span) |
| drag | pan; below the 4px slop threshold it is a click, not a drag |
| click | tune the selected channel to that frequency (no channel: tune the radio) |
| double-click | re-centre the radio on that frequency and reset the view |
| drag a marker | move that channel's offset |
| `Esc` / reset button | back to full span |

**Markers.** Each channel is a full-height hairline plus a passband band at its bandwidth, with
a label chip at the top carrying mode and offset. Unselected are `plot-ink-dim` hairlines; the
selected one is a 2px `plot-ink` line with an `accent` chip. Hit area is ≥12px wide and
invisible — the drawn line stays 1px.

**Toolbar.** Ghost controls in the plot's bottom-left — the one corner no data occupies —
appearing at rest and never animating: colormap, max-hold, and reset (which appears only once
the view is zoomed, since it has nothing to say at full span). The readout (centre of the
*visible* window · visible span · dB range) sits top-right. Both are drawn in the plot's own
achromatic ink, not the chrome palette.

---

## 10. Keyboard

`PLAN` §10 requires the client to be keyboard-first. One listener on the document, one table,
and every binding also listed in the `?` overlay — a shortcut nobody can find is not a feature.
These are the bindings that exist:

| key | action |
|---|---|
| `←` `→` | tune down / up one step |
| `Shift` + `←` `→` | tune ten steps |
| `[` `]` | smaller / larger tune step |
| `f` | type a frequency into the dial |
| `,` `.` | previous / next channel |
| `m` / `Shift+M` | cycle the selected channel's mode forward / back |
| `-` `=` | squelch down / up 2 dB |
| `s` | squelch on / off |
| `Space` | start / stop audio on the selected channel |
| `1`–`9` | switch view |
| `?` | this list |
| `Esc` | close an overlay or a menu |

Ownership rules, in the order they are applied:

1. `Ctrl` / `Cmd` / `Alt` combinations pass straight through — browser and OS shortcuts keep
   their meaning.
2. A text field, a `contenteditable`, or anything inside `[data-hotkeys="off"]` owns every key
   it receives. That attribute is how the dial keeps its own arrows and how every Base UI
   control that reads arrows, Home/End or typeahead letters keeps theirs.
3. A focused button or link owns `Space` and `Enter`.
4. Otherwise the table above applies, and a handled key calls `preventDefault` — `Space` would
   scroll and the arrows would move whatever the browser thinks is focused.

`1`–`9` selects a tab while the dockview shell still ships alongside the canvas; it re-targets
rack faces when the tabs are deleted in `CANVAS §8` phase ⑤. The selected node is what the
channel and mode bindings act on, and it is the same selection the canvas draws with an `accent`
border (§6).

---

## 11. States, focus, motion

**Every interactive element ships all its states**: rest, hover, `focus-visible`, active,
selected, loading, error, empty, unavailable. A control that cannot act right now stays enabled
and says why on use, unless the reason is obvious and imminent — a grey control with no
explanation is a dead end. The one licensed grey-out is §6's disconnected node, where the
subtitle carries the reason for the whole face at once.

**Focus** is a 2px `accent` ring at 2px offset, drawn with `:focus-visible` only, never removed.
Visual order = DOM order = tab order — inside a face that means the face's own reading order,
and a face must not reorder its DOM for layout. Popovers return focus to their trigger on close
and take `Esc`.

**Motion budget.** Transitions are ≤120ms and limited to colour and opacity on hover and focus.
No entrance animations — a node does not fade or scale in, a wire does not draw itself, a face
does not slide when pinned. No spinners in steady state, no pulsing, no animated wire "flow"
showing that data is moving; a live waterfall and a changing number are the evidence that data
is moving. A change of state may flash once (≤300ms) and then stop. `prefers-reduced-motion`
removes even those; it is a contract, not a suggestion.

**Latency.** Under 100ms feedback is immediate and needs no indicator; the WS-driven state
updates land inside that. Anything that can exceed 1s (a device open, an apply, a recording
stop) shows progress on the control that started it, in place.

---

## 12. Scope — what this edition does not fix

Named so they are not mistaken for oversights.

- **The light theme's port, category and accent tokens** (§2). Measured, prescribed, not yet in
  `index.css`; binding on the next change that touches it.
- **The `pointer-coarse:` variants in `controls.ts`** (§1). Vestigial; out on next touch.
- **Three sub-floor targets in `NodeShell`.** Resize handles (drawn and hit at 8px by React
  Flow's `NodeResizer`), port handles (`!size-2.5` — the 10px mark *is* the hit box), and the
  header pin / remove buttons (`size-5`, 20px) all sit under §4's 24px floor. Every drawn mark
  is the right size; the hit areas are not. Fix them the way spectrum markers were fixed — an
  invisible ≥24px grab area around the mark — when `NodeShell` is next opened.
- **No Playwright smoke flow.** `CANVAS §8` owes one for the canvas. The pure transforms — dial
  arithmetic, the view transform, axis ticks, the graph and rack operations — carry unit tests;
  composition is verified in a browser against `device-virtual`.
- **The band-plan explorer** (`PLAN` §8a) is a server feature; the dial and the scope are built
  so it can hang off them without rework.
- **`CANVAS §9`'s open questions** each have a design consequence still unwritten: how a scanner
  node shows that it owns a device's tuning, what a subgraph macro's face looks like, and
  whether a new channel node spawns docked to its device or floats free. Decide them in the
  phase that builds them, and add the numbers here in the same change.
