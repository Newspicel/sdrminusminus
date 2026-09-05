# Workspaces, templates, and presets

Choose what to save according to how much of the receiver you want to restore.

| Tool | Saves | Use for |
|---|---|---|
| Workspace | Patch, rack, device references and settings, band-plan choice | A complete receiver layout |
| Template | Built-in graph and radio configuration | Setting up a common activity |
| Preset | Named snapshot of a workspace and bound device settings | Restoring a tuned setup |
| Bookmark | Frequency and label | Retuning a selected device |

## Workspaces

Use the workspace name in the top bar to switch, create, or delete workspaces. The first workspace
in a new database contains a Device, Scope, and Speaker. Later workspaces start empty.

Changes are saved on the server, including node positions, rack layout, and band-plan region.
All connected clients share one active workspace.

### Export and import

The ↓ button beside a workspace downloads a JSON file containing its name, patch, rack, band-plan
choice, and node settings. The file excludes the database ID, revision, and undo history.

**Import a workspace file** creates and activates a new workspace. It never overwrites an existing
one; duplicate names receive a copy number. Available radios are opened with the imported settings.
Missing radios appear in the apply report, and their Device nodes wait for them. Select a replacement
radio to use different hardware.

Files from a newer, unsupported format version are rejected.

### Undo and redo

Use the top-bar arrows, `Ctrl`/`⌘ Z`, or `Ctrl`/`⌘ Shift Z`. Each workspace stores its last 100
layouts on the server, so undo and redo affect every connected client. Editing after undo discards
the redo history.

Undo also updates the running graph. Undoing an added channel closes it; redo recreates it with
its saved settings. Radio tuning is excluded from this history.

### Copy and paste

`Ctrl`/`⌘ C` copies selected nodes and the wires between them. `Ctrl`/`⌘ V` pastes and selects
the copies beside the originals. Repeated pastes are offset so they remain separate.

Connections to nodes outside the selection are excluded. A copied Device has no radio assigned;
select one before using it. The clipboard lasts for the browser tab's lifetime and works across
workspaces.

## Templates

Select a Device, then open **Library → Templates**. Applying a template immediately retunes that
radio, sets its sample rate, and merges channels and compatible displays or outputs into the workspace.
The apply button identifies the target radio.

Templates cover broadcast FM, airband, ADS-B, ACARS, AIS, APRS, paging, NAVTEX, radio clocks, GNSS,
marine VHF, PMR446, digital voice, ISM, DAB, and common amateur bands. Templates that the selected
radio cannot tune or sample are disabled.

Undo removes the added nodes, but does not restore the radio's previous tuning or sample rate.

## Presets

Save a preset after arranging and tuning a workspace. Applying it restores the graph and device
settings using durable radio identities where available. The apply report lists anything that
could not be restored.

Presets are editable and stored in the server database. Templates are read-only and ship with the app.

## Bookmarks and band plans

A bookmark saves a frequency and label. Applying it tunes the selected Device and keeps the graph.

**Bands** searches the active regional allocation data. Choose the region from the workspace menu
and enable the allocation ruler on Scope nodes if needed. Hover over the ruler for allocation
details; click to tune, using the band's usual mode when the data includes one.

Automatic region detection uses browser location and requires HTTPS or localhost. You can always
choose the region manually.
