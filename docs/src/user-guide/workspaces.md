# Workspaces, templates, and presets

sdr-- provides several ways to save receiver state. They overlap deliberately, but each answers a
different question.

| Tool | Saves | Best for |
|---|---|---|
| Workspace | Patch, rack, device references and settings, band-plan choice | A complete operating bench |
| Template | Built-in graph and radio configuration | Starting a common activity quickly |
| Preset | A named snapshot of the current workspace and bound device settings | Returning to a known station setup |
| Bookmark | A frequency and label | Tuning a selected device without replacing the graph |

## Workspaces

Use the workspace name in the top bar to switch, create, or delete workspaces. The first workspace
on a new database includes a starter Device, Scope, and Speaker. Later workspaces begin empty.

Workspace changes are saved on the server, including node position, rack layout, and regional band
plan. One workspace is active at a time across every connected client.

### Undo and redo

The arrow buttons in the top bar step the workspace back through its changes, or forward again.
`Ctrl`/`⌘ Z` and `Ctrl`/`⌘ Shift Z` do the same from the keyboard.

The history is stored with the workspace rather than in the browser, so every connected client
shares one list: undoing in one window undoes for all of them, and a step another operator takes
is the one your buttons offer next. Each workspace keeps its last 100 arrangements, and editing
after an undo discards the steps that were ahead of it.

A step brings the hardware with it. Undoing a change that added a channel closes that channel, and
redoing it opens it again with the settings it had. Where a radio is tuned is not part of the
history — undo restores what the workspace draws, not the dial.

### Copy and paste

`Ctrl`/`⌘ C` copies the selected nodes on the patch, together with the wires that run between
them. `Ctrl`/`⌘ V` pastes them beside the originals and selects the copies, so the next drag moves
what was just pasted. Pasting again leaves a second copy rather than stacking one on the other.

Wires leaving the selection are not copied — they name nodes the copy does not carry. A copied
Device names no radio: a radio is opened once and belongs to one node, so choose the radio for the
new Device node yourself. The clipboard lasts as long as the browser tab, so a chain copied from
one workspace can be pasted into another.

## Templates

Open **Library → Templates** after selecting a Device node. A template retunes that radio, sets an
appropriate sample rate, adds its channels, and merges the necessary displays or outputs into the
workspace.

Built-in templates cover FM broadcast, civil airband, ADS-B, ACARS, AIS, APRS, POCSAG, NAVTEX,
radio clocks, GNSS, 2 m amateur radio, marine VHF, PMR446, 70 cm digital voice, the 433 MHz ISM
band, DAB blocks, and the 20 m HF digital, keyboard and SSTV segments. A template wires each of
its channels only to the sinks that channel can feed, so a decoder that has no audio never lands
on a speaker. The server disables templates that the selected radio cannot tune or sample
correctly.

Applying a template changes live device and channel configuration immediately. The button names
the target radio because this is not a preview: undo removes the nodes the template merged in, but
the radio keeps the sample rate and tuning the template gave it.

## Presets

Create a preset after arranging and tuning a workspace you want to recall. Applying it reconciles
the saved graph and device settings with currently attached radios. Durable device identities are
used where possible, and the apply report explains anything that could not be restored.

Presets are writable and local to the server database; templates are read-only and ship with the
application.

## Bookmarks and band plans

A bookmark records a useful frequency and label. Applying one tunes the selected Device without
rebuilding the rest of the workspace.

The **Bands** section searches the active regional allocation data. Set the region from the
workspace menu and choose whether Scope nodes draw the allocation ruler. Automatic location
detection requires HTTPS or localhost because browsers block geolocation on an insecure LAN
origin; choosing the region manually always works.
