# DESIGN.md — the visual and interaction system for sdr--

This file is binding the way `CLAUDE.md` is binding. `PLAN.md` §10 says *what* the client must
be; this says *how it looks and behaves*, in numbers a reviewer can check. If a change needs a
value that is not here, add it here in the same change.

The reference points are the ones §10 names: bench instruments and pro audio, not landing pages.
Everything below follows from three commitments.

1. **The plot is the instrument.** The spectrum and waterfall own the screen and the colour
   channel. Chrome is the bezel around them: quiet, dense, and out of the way.
2. **Colour is a budget, not a decoration.** A hue only appears where it carries meaning
   (§4.13 of the geometry guide: every decorative saturated colour is subtracted from alarm
   salience). The waterfall's colormap spends nearly the whole budget, so the chrome runs on
   warm neutrals and one accent.
3. **Nothing moves that the operator did not move.** Steady state is zero motion. A live
   waterfall is data, not animation.

---

## 1. Direction

**Anodized.** Warm graphite surfaces, silkscreen legends, one lamp-amber accent — the vernacular
of an anodized-aluminium front panel with a backlit dial. The previous palette (teal on blue-
black) was the generic dark-dashboard default; warm neutrals separate the chrome from the plot,
which is where all the blue-violet already lives in the magma colormap.

**Signature element: the dial.** The frequency readout is a machined tuning dial, not a label.
Each digit is its own control — hover it, scroll it, arrow it, type over it. It is the largest
thing in the chrome, the only place with a display-scale type size, and the first thing the eye
lands on. Everything else in the top bar is deliberately quieter so the dial is unmistakable.

---

## 2. Colour

Authored in OKLCH, one hue family per role, with lightness carrying elevation (§3.4: on dark
surfaces the shadow channel is algebraically bankrupt, so depth is surface lightness).
The light theme is the same role table re-anchored, never a hand-picked second set of hexes.

### Surfaces and ink

| role | dark | light | job |
|---|---|---|---|
| `bg` | `oklch(.185 .006 75)` | `oklch(.955 .004 80)` | app ground, elevation 0 |
| `panel` | `oklch(.225 .007 75)` | `oklch(.985 .003 80)` | panels, bars — elevation 1 |
| `panel-2` | `oklch(.265 .008 75)` | `oklch(.925 .005 80)` | control fills, wells — elevation 2 |
| `panel-3` | `oklch(.305 .009 75)` | `oklch(1 0 0)` | popovers, menus — elevation 3 |
| `line` | `oklch(.34 .008 75)` | `oklch(.865 .006 80)` | separators between regions |
| `line-strong` | `oklch(.52 .009 75)` | `oklch(.60 .008 80)` | control borders — clears 3:1 non-text |
| `ink` | `oklch(.92 .008 80)` | `oklch(.27 .012 75)` | primary text, values |
| `ink-dim` | `oklch(.70 .010 80)` | `oklch(.47 .012 75)` | labels, secondary text |
| `ink-faint` | `oklch(.60 .010 80)` | `oklch(.53 .012 75)` | micro-legends, units |

Elevation step is ΔL ≈ 0.04. Dark themes never use pure black (`bg` sits at L .185, inside the
.18–.24 halation-safe band) and never use pure white text (`ink` at L .92).

### Semantic

| role | dark | light | meaning |
|---|---|---|---|
| `accent` | `oklch(.80 .135 72)` | `oklch(.56 .125 62)` | interactive, selected, focused, tuned |
| `accent-dim` | `oklch(.62 .10 72)` | `oklch(.68 .10 62)` | accent at rest / underlays |
| `danger` | `oklch(.70 .16 27)` | `oklch(.51 .19 27)` | faults, destructive, rejected |
| `ok` | `oklch(.78 .14 155)` | `oklch(.50 .115 155)` | live, playing, recording-healthy |

Hue separation accent 72° / danger 27° / ok 155° holds in both themes, and **no state is carried
by hue alone** (§3.6) — every one pairs with a word, a glyph or a position.

Measured contrast, dark on `bg`: ink 14.7:1, ink-dim 7.0:1, ink-faint 4.7:1, accent 9.8:1,
danger 6.5:1, ok 9.9:1. Light on `bg`: ink 13.2, ink-dim 6.0, ink-faint 4.6, accent 4.2,
danger 5.6, ok 5.0. Every one clears the 4.5:1 floor; dark carries the extra margin the
polarity-blindness of the WCAG formula demands.

### Plot ink — the rule that keeps the waterfall readable

**The plot never inverts and its overlays are achromatic.** The colormap owns hue inside the
plot rectangle, so anything drawn on top of it separates by *luminance and shape* only:
isoluminant edges are nearly invisible, and a coloured cursor over a colormap is a conjunction
search. The plot therefore has its own token set, identical in both themes:

| token | value | job |
|---|---|---|
| `plot-bg` | `oklch(.16 .008 75)` | behind the trace and unwritten waterfall |
| `plot-grid` | `oklch(.55 .006 80)` @ 14% | gridlines, always lighter-weight than data |
| `plot-trace` | `oklch(.93 .020 85)` | the live spectrum line |
| `plot-hold` | `oklch(.62 .015 85)` | max-hold trace |
| `plot-ink` | `oklch(.97 .010 85)` | selected marker, axis text |
| `plot-ink-dim` | `oklch(.72 .010 85)` | unselected markers, tick labels |

The one licensed exception is the *label chip* on a marker: it sits on its own opaque `bg`
plate, outside the colormap, so it may carry `accent` to mark selection.

### Waterfall colormaps

All shipped colormaps are perceptually uniform and monotone in luminance: jet and its
relatives are excluded on purpose. `magma` is the default; `inferno`, `plasma`, `viridis` and
`gray` are selectable. The choice is a per-eye preference, so it lives in `localStorage`, not
in the workspace. The waterfall advances one history row per CSS pixel, so its scroll rate is
the frame rate and no arriving row is ever skipped.

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
  cosmetic one.
- **Legends:** 10px uppercase, `0.09em` tracking, `ink-faint` — the silkscreen voice. Used for
  units, column heads and section labels; never for values.

Scale (ratio ≈ 1.25, base 13px — dense operational surface):

```
10  legend      12  control/table      13  body (base)      16  panel heading
20  emphasis    26  dial (compact)     34  dial (≥ md)
```

Line height: 1.5 body, 1.2 headings, fixed px for table rows. Numeric columns are
right-aligned with uniform precision per column and reserve width for their live maximum.

---

## 4. Space, separation, density

Spacing scale `2 4 8 12 16 24 32 48`. Every margin, padding and gap comes from it.

**Proximity is a ratio.** Between-group spacing ≥ 2× within-group. A divider is only drawn
where that ratio cannot be afforded — which is the test for every border in the UI.

**Separation ladder**, use the least that does the job: spacing → tint (`panel-2` well) →
hairline (`line`) → shadow. Shadow means *elevation*, so it appears only on popovers, menus and
drag ghosts. On dark surfaces those also get `+ΔL` surface lightness (`panel-3`) and a 1px top
highlight (`inset 0 1px 0 oklch(1 0 0 / .06)`), because a drop shadow on an L .2 ground cannot
reach popover-grade edge contrast at any alpha.

**Density: compact.** This is a monitoring surface an operator watches for hours. Control
height 28px, table row 30px, panel padding 12px. The floor is absolute: on coarse pointers
every interactive element grows to ≥ 40px, and no target is ever below 24px on any pointer —
compact buys space by trimming padding, never hit area.

Radii are concentric, not equal: popover 6px, control 3px, chip 3px, plot 0.

---

## 5. Layout

Two rows of chrome, then the dock. The previous shell stacked five.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ sdr--  │ 1 0 0 . 0 0 0  0 0 0 MHz │ ‹ step 100k ›  │ RADIO ▾ │ REC │  ⟳ ● live │  56px
├──────────────────────────────────────────────────────────────────────────────┤
│ Station ▾ │ Overview  Decoders  +  │                            + Panel ▾ │ ☀ │  32px
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│                              the dock                                        │
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Top bar** = the radio. Dial, tune step, the radio popover (rate, bandwidth, gains, PPM,
  antenna, driver extras, close), record, link state. Everything that changes what the hardware
  is doing, and nothing else.
- **Tab bar** = the view. Workspace menu, tab strip, add-panel. Everything that changes what
  you are looking at, and nothing else.
- The device-settings strip, the first-run banner and the workspace name/add/remove row are
  gone as rows: settings moved into the radio popover, first-run became the spectrum's
  no-device empty state, workspace management became the workspace menu.
- **Errors never take a row.** They are toasts in the bottom-right, dismissible, auto-expiring
  for the transient kinds. A banner that appears and disappears at the top of a layout moves
  every panel underneath it, and the operator did not move them.
- Bars are flush to the viewport edges: the far-right controls sit against the edge, where the
  cursor cannot overshoot them.

---

## 6. The dial

`FrequencyDial` renders the tuned centre as `100.000 000 MHz`, grouped MHz / kHz / Hz. Leading
zeros are drawn in `ink-faint` so magnitude is readable before any digit is parsed.

| gesture | effect |
|---|---|
| wheel over a digit | ± one unit of that digit's decade |
| click a digit | focus it (the dial is one tab stop; ←/→ move between digits) |
| ↑ / ↓ | ± one unit of the focused digit |
| PageUp / PageDown | ± ten units |
| 0–9 typed | write that digit and advance right |
| Enter or `f` from anywhere | open direct entry — type `145.5`, `145m5`, `433800k` |
| Esc | cancel direct entry, restore the tuned value |

The active digit carries a 2px `accent` underline; the whole dial carries the focus ring when
focused. Every step clamps to the device's `frequency_range`, and a clamped step is silent —
the value simply stops, which is the honest report that the radio cannot go further.

All of it is arithmetic on integer Hz in `frequencyDial.ts`, unit-tested. The component only
routes events.

---

## 7. The spectrum

The centrepiece, and the part that was furthest from the reference tools.

**Regions.** Trace on top, waterfall below, a draggable 1px divider between them (12px hit
area). The split fraction is client state. Frequency axis on the divider, dB scale in the
trace's left gutter, both drawn from the frame's own metadata.

**View transform.** Pan and zoom are a client-side window `[start, end] ⊆ [0,1]` over the
device span — the server streams a fixed span, so zooming magnifies rather than resolves, and
the readout says so by showing the *visible* span. Wheel zoom is the fixed-point affine
transform: the frequency under the cursor stays under the cursor. Pure math in
`spectrumView.ts`, unit-tested.

| gesture | effect |
|---|---|
| wheel | zoom about the cursor (1.2× per notch, clamped to 512× and to full span) |
| drag | pan; below the 4px slop threshold it is a click, not a drag |
| click | tune the selected channel to that frequency (no channel: tune the radio) |
| double-click | re-centre the radio on that frequency and reset the view |
| drag a marker | move that channel's offset |
| `Esc` / reset button | back to full span |

**Markers.** Each channel is a full-height hairline plus a passband band at its bandwidth, with
a label chip at the top carrying mode and offset. Unselected are `plot-ink-dim` hairlines;
the selected one is a 2px `plot-ink` line with an `accent` chip. Hit area is ≥ 12px wide (40px
on coarse pointers) and invisible — the drawn line stays 1px.

**Toolbar.** Ghost controls in the plot's bottom-left — the one corner no data occupies —
appearing at rest and never animating: colormap, max-hold, and reset (which appears only once
the view is zoomed, since it has nothing to say at full span). The readout (centre of the
*visible* window · visible span · dB range) sits top-right. Both are drawn in the plot's own
achromatic ink, not the chrome palette.

---

## 8. Keyboard

`PLAN` §10 requires tune step, mode, squelch and tab switching to be bound. Handlers ignore
events whose target is a text input, and every binding is listed in the `?` overlay — a
shortcut nobody can find is not a feature.

| key | action |
|---|---|
| `←` `→` | tune ∓ / ± one step |
| `Shift` + `←` `→` | ten steps |
| `[` `]` | smaller / larger tune step |
| `f` | focus the dial for direct entry |
| `m` / `Shift+M` | cycle the selected channel's mode forward / back |
| `-` `=` | squelch ∓ / ± 2 dB on the selected channel |
| `s` | squelch on / off |
| `Space` | start / stop audio on the selected channel |
| `1`–`9` | switch to that tab |
| `,` `.` | previous / next channel |
| `?` | shortcut overlay |
| `Esc` | close the overlay / popover, or reset the spectrum view |

---

## 9. States, focus, motion

**Every interactive element ships all its states**: rest, hover, `focus-visible`, active,
selected, loading, error, empty, unavailable. A control that cannot act right now stays enabled
and says why on use, unless the reason is obvious and imminent — a grey control with no
explanation is a dead end.

**Focus** is a 2px `accent` ring at 2px offset, drawn with `:focus-visible` only, never removed.
Visual order = DOM order = tab order. Popovers return focus to their trigger on close and take
`Esc`.

**Motion budget.** Transitions are ≤ 120ms and limited to colour and opacity on hover/focus.
No entrance animations, no spinners in steady state, no pulsing. A change of state may flash
once (≤ 300ms) and then stop. `prefers-reduced-motion` removes even those; it is a contract,
not a suggestion.

**Latency.** Under 100ms feedback is immediate and needs no indicator; the WS-driven state
updates land inside that. Anything that can exceed 1s (a device open, a recording stop) shows
progress on the control that started it, in place.

---

## 10. Scope — what this pass does not do

Named so they are not mistaken for oversights:

- **Per-channel dock panels** and **panels pinned to a radio** stay out. Both need identity that
  survives an engine restart (`PROGRESS.md`), which is a server change, not a UI one.
- **The band-plan explorer** (`PLAN` §8a) is a server feature; the dial and the spectrum are
  built so it can hang off them later without rework.
- **Layouts inside templates** remain a `PLAN` §10 open item.
- **No Playwright smoke flow.** The pure transforms — dial arithmetic, view transform, axis
  ticks — carry unit tests; the composition was verified in a browser against `device-virtual`.
