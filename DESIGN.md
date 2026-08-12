# DESIGN.md — the visual and interaction system for sdr--

This file is binding the way `CLAUDE.md` is binding. 

This is the canvas edition (`CANVAS §8` phase ②). It replaces the edition written for the
top-bar / tab-bar / dock shell. Two things are gone, not deprecated: the anodized single-accent
chrome as an organising idea — colour is now spent first on **what a wire carries** — and every
mobile and touch rule, because `PLAN §18` removed mobile support outright. What survived
survived because it is function, not paint: the contrast floors, the plot-ink discipline, mono
tabular numerals, the density scale, and "nothing moves that the operator did not move."

The reference points are the ones `PLAN` §10 names, now joined by the ones `CANVAS` names: bench
instruments, pro audio, and a modular rack — not landing pages, and not GRC. Everything below
follows from three commitments.

1. **The patch is the instrument.** The workspace is a graph the operator laid out, and the
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
> `pointer-coarse:min-h-10` / `pointer-coarse:size-10` variants, and `Slider.tsx` a
> `pointer-coarse:h-10` whose comment still justifies itself by the 40px floor deleted above.
> Neither file was touched by the canvas rewrite; both are vestigial under the rule above and
> come out the next time they are opened.

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

> **Open defect, not a deferral.** The light `accent` above is `oklch(.545 .125 62)` = 4.51:1;
> `web/src/index.css` ships `oklch(.56 .125 62)` = 4.23:1, which fails the text floor wherever
> the accent is text — `segment()` selected, `BTN` hover. The row above is the fix, and §12
> carries it until it lands. The previous edition of this document printed 4.2 and claimed the
> floor was clear; that claim was wrong and is not carried forward.

### Hue = data type

The rule, and it has no exceptions: **outside a plot rectangle, hue on a port, a wire or a
header strip encodes the type of thing that flows, and nothing else.** It never encodes state,
health, freshness, selection, ownership or severity — those are the four semantic roles above,
and each of them is paired with a word or a glyph. A hue that starts meaning two things has
stopped meaning either.

Every port therefore ships **hue + a marker shape + a text label**. The shape is what a
colourblind operator reads; the label is the accessible name and the hover title. With colour
removed entirely the graph must still be unambiguous — that is the acceptance test.

| type | carries | token (dark) | token (light) | marker | dark vs `bg` | light vs `bg` |
|---|---|---|---|---|---|---|
| `iq` | wideband complex baseband at the device rate | `oklch(.72 .11 235)` | `oklch(.592 .11 235)` | filled circle | 7.7:1 | 3.5:1 |
| `audio` | 48 kHz demodulated audio (Opus on the wire) | `oklch(.74 .12 158)` | `oklch(.583 .12 158)` | diamond (square rotated 45°) | 8.5:1 | 3.5:1 |
| `events` | typed decoder frames (`DecodedRecord`) | `oklch(.78 .11 85)` | `oklch(.599 .11 85)` | square | 9.3:1 | 3.5:1 |
| `video` | scanned pictures, one raster per field | `oklch(.76 .12 35)` | `oklch(.61 .12 35)` | triangle, standing on its base | 8.3:1 | 3.5:1 |
| `control` | tuning ownership — a scanner driving a radio | `oklch(.76 .1 300)` | `oklch(.606 .1 300)` | arrowhead (triangle, pointing right) | 8.4:1 | 3.5:1 |
| `tx` | baseband to be transmitted, **reserved** (`PLAN §12a`) | `oklch(.76 .12 345)` | `oklch(.613 .12 345)` | hollow circle | 8.2:1 | 3.5:1 |

Five of the six are things that move today. `tx` is the one reservation, and it is drawn as what
it is: `iq`'s own circle, going the other way, **left unfilled because nothing fills it** — no node
kind emits that type, so the port refuses every wire and says so in its hover title. It earns a row
here rather than waiting, because the device node's shape is what tells an operator a radio has a
send side at all — and it says that per *radio*, not per node kind: the input is drawn only where
the radio has a transmit side (`PortCondition::DeviceIsTxCapable`, read off `Capabilities.duplex`),
so an RTL-SDR node has two ports and a
transceiver has three. A receiver never shows a socket its hardware does not have. `iq-tap`
(decimated channel IQ) and `position` (GPS) get no such reservation and stay out until their
features land: nothing is holding a place for them.

A wire and a marker are non-text graphics, so the floor is 3:1 against the ground they are drawn
on — the canvas `bg` for a wire, `panel` or `panel-2` for a marker. The columns above are measured
against each theme's `bg`; against `panel-2` (the tightest ground a marker sits on) the five
measure 6.3 / 7.0 / 7.6 / 6.9 / 6.8:1 dark, and 3.2:1 each in light.

Both themes are now written out in `index.css`. They were not: the tokens lived only in `@theme`,
so the light block inherited the dark values and measured 1.8–2.1:1 on a light ground — a patch
that could not be read in that theme at all. The category strip below had the same defect and is
re-anchored with them.

Port chroma is held ≤ .12 and category chroma ≤ .07, both under `accent`'s .135, so a lit wire
never outshouts a tuned control.

### Wires

- Drawn 1.5px in the hue of the port they leave.
- Hover and selection **brighten**: 2.5px and `brightness(1.35)`. They never recolour — the hue
  is already saying what the wire carries, and a wire that changed colour on hover would be
  hue carrying state.
- The in-flight connection line is 2px `accent`, because it is not carrying anything yet.
- A **refused** wire is never drawn — the port types do not join, or the input already has its
  one wire. The reason is stated in words where the operator is looking (`CANVAS §1`): a
  bottom-right toast, raised from `onConnectEnd`, because React Flow only calls `onConnect` for
  connections it has already accepted.
- A **faulted** wire *is* drawn: `danger`, 5-4 dashed, with a short label on it (`needs 2.000
  MHz`). This is the wire that is legal but cannot carry what it says it carries — a wideband
  mode on a receiver at the wrong rate (`PLAN §18`). It is a fault and not a refusal because the
  rate is one setting away: the operator meant to put that decoder on that radio, and the face
  at the end of the wire says why in full and offers the setting as a button. Refusing the
  connection would have made the patch unable to express an intention the workspace can satisfy.

### Category strip

A 4px strip on the node header says what the box *is* before its label is read. Low chroma:
this is a silkscreen mark, not a status light. Measured against `panel-2`, the header ground:

| category | dark | light | vs `panel-2`, dark | vs `panel-2`, light |
|---|---|---|---|---|
| `source` | `oklch(.62 .07 235)` | `oklch(.594 .07 235)` | 4.3:1 | 3.2:1 |
| `channel` | `oklch(.62 .07 300)` | `oklch(.603 .07 300)` | 4.1:1 | 3.2:1 |
| `display` | `oklch(.62 .05 200)` | `oklch(.592 .05 200)` | 4.3:1 | 3.2:1 |
| `feature` | `oklch(.62 .07 60)` | `oklch(.602 .07 60)` | 4.1:1 | 3.2:1 |
| `sink` | `oklch(.58 .03 80)` | `oklch(.598 .03 80)` | 3.6:1 | 3.2:1 |

All clear the 3:1 non-text floor, in both themes now that `--color-cat-*` is written out in the
light block as well. The strip is never the only thing distinguishing two nodes: the title, the
ports and the face say it too.

### Plot ink — the rule that keeps the waterfall readable

**The plot never inverts and its overlays are achromatic.** The colormap owns hue inside the
plot rectangle, so anything drawn on top of it separates by *luminance and shape* only:
isoluminant edges are nearly invisible, and a coloured cursor over a colormap is a conjunction
search. Data-type hue stops at the plot's edge for the same reason. The plot has its own token
set, identical in both themes:

| token | value | job | vs `plot-bg` |
|---|---|---|---|
| `plot-bg` | `oklch(.16 .008 75)` | behind the trace and unwritten waterfall | — |
| `plot-grid` | `oklch(.55 .006 80)` @ 16% | gridlines, always lighter-weight than data | — |
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
property of the eye looking at the screen, not of the workspace, so unlike the patch itself it does not
sync between clients.

The control is **one icon that cycles** them, not three permanent segments: the bar is for
operating a radio, and a choice made twice a year does not hold a fifth of it. The guess a bare
cycling icon leaves — *which state am I in?* — is closed by the glyph (sun, moon, split disc) and
by the label naming the current state before the next one: "Theme: Auto. Switch to Dark".

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
radio. Absence is a state of the workspace, not a fault: it takes no `danger` hue, no toast and no
badge. The dimming plus the subtitle is the whole report.

**A face that cannot say anything at 220 × 140 is too big a face.** Give it a summary state at
minimum size and its detail above that, or split it into two nodes.

---

## 7. The face

The face is the instrument, and it is the only control surface — there is no settings dialog
behind it (`CANVAS §1`). Signature: `XFace({ node }: { node: PatchNode })`, wrapped in
`NodeShell`, with live state read from the workspace context and never from props.

**Gesture ownership.** Inside the face rectangle the instrument owns the wheel and the drag: a
scope zooms about the cursor, a dial digit steps, a slider drags. The canvas pans and
rubber-bands only from the pane, and a node moves only by a drag on its header or on inert body
space. Any control inside a face that itself takes a pointer drag or a wheel must opt out of the
canvas gestures (`nodrag` / `nowheel`), or the operator will move the node while trying to set a
value. This is the rule that keeps §5's camera and §9's plot gestures from claiming the same
event.

**Scrolling.** `FaceBody` scrolls its content by default; `scroll={false}` when the content owns
its own size — a plot, a map, a canvas.

**Pinning adds a surface; it never removes one** (`CANVAS §5`). A pinned face renders in the
rack *and* keeps its place on the canvas: a node that turned into a placeholder left a hole
where the operator had put an instrument, and made the patch a worse picture of the workspace for
having operated it. The patch and the rack are alternate views, so only one is mounted at a
time.

**One GL context for every plot** (`CANVAS §7`). Browsers cap live GL contexts at roughly 8–16,
so every scope face shares one context and one renderer, whichever view it is in. GL faces
render only while on screen, and because React Flow zooms with a CSS transform they re-render at
a zoom-adjusted device pixel ratio — a zoomed plot is redrawn crisp, never upscaled as a bitmap.
MapLibre is the exception the budget still has to respect: it takes a context per map instance,
so a view showing the canvas and the rack at once would pay twice for a pinned map.

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
| default slot | 12 × 8 cells — six fit the grid, two across by three down |
| minimum slot | 1 × 1 cell |
| drag / resize | snap to whole cells; a move that would overlap or leave the grid is refused in place, never pushed or reflowed |
| pin placement | first free cell, scanning left-to-right then down |
| full rack | the pin is a no-op — a full rack is a rack, not an error |
| deleted node | its slot is dropped |

Pinning leaves the canvas node exactly where it was (§7); unpinning only takes the face off the
grid. The rack may be empty — the canvas alone is a complete UI, and the rack is never a required stop.
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
| Enter | open direct entry — type `145.5`, `145m5`, `433800k`. Keyboard only: a pointer gesture that opened it swallowed the second press of a double-click on a digit, which is the fastest way to step one |
| Esc | cancel direct entry, restore the tuned value |

The active digit carries a 2px `accent` underline; the whole dial carries the focus ring when
focused. Every step clamps to the device's `frequency_range`, and a clamped step is silent — the
value simply stops, which is the honest report that the radio cannot go further. All of it is
arithmetic on integer Hz in `dial.ts`, unit-tested; the component only routes events.

### The scope — one component, patched anywhere

On a device's `iq` it is the band view; on a channel's tap it is the channel analyzer
(`CANVAS §1`). Trace on top, waterfall below, a draggable 1px divider between them whose grab
strip is ≥12px — §4's licence for a 1px-adjacent mark, and the required number, not the shipped
one (§12). The split fraction is client state. Frequency axis on the divider, dB scale in the
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

### 9a. The band ruler — the one place hue leaves the data

`FEATURES §5`'s allocation layer, drawn as a gutter above the trace and sharing its width, so the
ruler's axis and the plot's axis are the same axis. Two rows of 16px, one per lane the server
resolves: the regulatory stack merged most-specific-wins, and each amateur band plan as an
overlay. Toggled by a `bands` button in the plot toolbar, off-state remembered per browser
beside the colormap.

**Why it is not an overlay.** §2 says the colormap owns hue inside the plot rectangle and every
overlay on it is achromatic. A ruler whose whole job is to separate ten services at a glance
cannot obey that and still work, so it does not go inside the rectangle — it goes beside it, on
its own opaque `bg` strip, which is the same licence the marker label chip already has. Nothing
the ruler knows is ever projected into the plot.

**Ink.** Each block is its service hue at 25% over `bg`, with a 1px full-chroma rule at the left
edge *only where the band actually starts* — an edge at the window's rim would claim a boundary
that is only where the screen ran out. The band's name is written across the block in `ink` at
10px when the block is at least 7% of the window; narrower blocks are read through the popover.
The ten `--color-band-*` roles are re-anchored per theme like the port and category families, so
each edge rule clears the 3:1 non-text floor with margin. Measured against `bg`:

| | ism | broadcast | mobile | science | maritime | aeronautical | navigation | amateur | satellite | other |
|---|---|---|---|---|---|---|---|---|---|---|
| dark | 5.30 | 5.40 | 5.75 | 5.77 | 5.77 | 5.65 | 5.51 | 5.35 | 5.27 | 4.72 |
| light | 4.59 | 3.88 | 4.06 | 3.89 | 3.72 | 3.99 | 4.32 | 4.28 | 4.23 | 3.76 |

The light theme is the tighter one, as everywhere in this file, and its floor is `maritime` at
3.72:1. Lightness is uniform within a theme and the spread is the hue's own luminance, which is
why the warm and green hues sit lowest.

Ten hues is more than any other family in this file carries, and 25° is the smallest gap between
two of them. That is why §2's "no state by hue alone" is load-bearing here and not decorative:
every block states its service in words in the identify popover and again in the explorer, and
the hue is a grouping cue, never the identification.

**Gestures.** The ruler is `data-plot-chrome`, so the plot declines it: a click identifies, and
it can never retune a running radio the way a click on the trace does. The popover names the
frequency, then one section per lane — band, service, authority, edges, channel step, notes —
and unrolls everything the winner covers, which is the whole point of resolving the layers
rather than flattening them. Two things it shows because the tables are now *generated* from the
regulators' own documents: the official wording under the operator's name (`MOBILER
SEEFUNKDIENST` under "Marine VHF") and the row's identifier in its source (`Eintrag 27001`), so
a claim on this ruler can be checked against the publication it came from. `also X` marks a
co-allocation from the same layer, `over ITU: X` a layer underneath; a secondary allocation
carries a chip saying so, because it means "you may use this if nobody else is". Tuning is the
one explicit button, and it carries the band's suggested mode in its label so the click is never
a surprise.

**Keyboard.** The ruler is a pointer instrument; a lane row is one target, not one per band. The
band explorer in the library drawer is the keyboard route to the same table — searchable, and
every result tunable. The region and the ruler's on/off are not chosen there: they are workspace
settings (`WorkspaceSettings`, stored in the snapshot) and live under the workspace menu with the
rest of them, because which plan is in force is a property of the bench and not of the browser
looking at it. Sub-band targets on the ruler itself
are not attempted: at a 2.4 MHz span a 25 kHz channel is a quarter of a pixel, and a target that
small is a worse answer than a list.

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
| `f` | focus the selected receiver's dial — then Enter to type a frequency |
| `,` `.` | previous / next channel |
| `m` / `Shift+M` | cycle the selected channel's mode forward / back |
| `-` `=` | squelch down / up 2 dB |
| `s` | squelch on / off |
| `Space` | start / stop audio on the selected channel |
| `1`–`9` | select the nth node of the patch |
| `p` | pin / unpin the selected face on the rack |
| `v` | swap the patch and the rack |
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

`1`–`9` counts the patch in stored order, so a number key is a fixed address for a node the
operator can see. The selected node is what the channel and mode bindings act on, and it is the
same selection the canvas draws with an `accent` border (§6). This table and `BINDINGS` in
`useHotkeys.ts` are one list in two places — the `?` overlay renders `BINDINGS` verbatim — so a
binding added there is added here in the same change.

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

- **The light theme's `accent`** (§2). Measured and prescribed there at `oklch(.545 .125 62)`,
  two characters short in `index.css` at `.56` = 4.23:1 — under the 4.5:1 text floor wherever the
  accent *is* text (`segment()` selected, `BTN` hover). The port and category halves of this
  defect are fixed: both token families are written out in the light block, so the 3:1 non-text
  floor is clear on every wire and strip. This third one was inherited, not introduced here, and
  is still open.
- **The `pointer-coarse:` variants in `controls.ts` and `Slider.tsx`** (§1). Vestigial under
  desktop-only; neither file has been touched since the rule was deleted, so they come out on
  next touch.
- **Three sub-floor targets in `NodeShell`.** Resize handles (drawn and hit at 8px by React
  Flow's `NodeResizer`), port handles (`!size-2.5` — the 10px mark *is* the hit box), and the
  header pin / remove buttons (`size-5`, 20px) all sit under §4's 24px floor. Every drawn mark
  is the right size; the hit areas are not. Fix them the way spectrum markers were fixed — an
  invisible ≥24px grab area around the mark — when `NodeShell` is next opened.
- **The scope's trace/waterfall divider grabs at 9px** (`ScopeFace.tsx`, `h-[9px]`) where §4
  licenses ≥12px around a 1px-adjacent mark and §9 states the rule. Same shape as the three
  above: the ink is right, the grab strip is short.
- **The band-plan explorer** shipped (§9a): the tables and the layering are the server's, and the
  dial and the scope did in fact take it without rework. What is still open in it is stated
  there — the ruler has no per-band keyboard target, and the region is guessed from a bounding
  box rather than a boundary.
- **A subgraph macro's face** — a saved patch fragment placed as one node — is undesigned. Decide
  it in the phase that builds it, and add the numbers here in the same change. (The other two
  questions this entry used to carry are answered: a scanner shows its ownership as the control
  wire running into the radio it drives, and a new node lands to the right of everything already
  drawn rather than docked to anything.)
- **What a device node looks like once it can transmit** (`PLAN §12a`). The reserved `tx` input is
  drawn on the radios that have one; the send half of the face behind it — power, keying, the
  on-air indicator, the authorized-use gate — is not designed. (The question this entry used to
  carry alongside it is answered: per-radio gating of the port. The claim that a static catalog
  cannot say a bare transmit flag was the wrong shape of answer — the catalog does not have to *say* it,
  only to state that the port depends on it, which is what it already did for a channel's
  conditional outputs. The port table now carries `PortCondition::DeviceIsTxCapable` and the
  client resolves it against the binding, so an unbound or receive-only node has no transmit
  input at all.)
