# Workspaces, templates, and presets

| Tool | Saves or supplies | Use |
|---|---|---|
| Workspace | Patch, rack, radio references and settings, band plan | A complete receiver layout |
| Template | Built-in graph and radio configuration | Start a common activity |
| Preset | Named workspace snapshot with bound radio settings | Restore a tuned setup |
| Bookmark | Frequency and label | Retune a selected Device |

## Workspaces

Use the workspace name in the top bar to create, switch, or delete layouts. A new database starts
with Device, Scope, and Speaker nodes; later workspaces start empty.

Changes save automatically on the server. All clients share the active workspace.

### Export and import

The ↓ button downloads the workspace as JSON, including its name, patch, rack, band plan, and
node settings. Database identity, revision, and undo history are excluded.

**Import a workspace file** creates and activates a new workspace. Duplicate names receive a
copy number. Available radios open with the imported settings; missing radios remain disconnected
and appear in the apply report. Select a replacement to use different hardware.

Unsupported newer file versions are rejected.

### Undo and redo

Use the top-bar arrows or `Ctrl`/`⌘ Z` and `Ctrl`/`⌘ Shift Z`. Each workspace keeps 100 layouts
on the server. Undo affects all clients and updates the running graph; for example, undoing an
added channel closes it. A new edit after undo discards redo history.

Radio tuning is excluded from layout history.

### Copy and paste

Select nodes, press `Ctrl`/`⌘ C`, then `Ctrl`/`⌘ V`. Copies appear beside the originals with their
internal wires. Connections outside the selection are excluded, and copied Device nodes need
a radio selected.

The clipboard works across workspaces for the lifetime of the browser tab.

## Templates

Select a Device, then open **Library → Templates**. Applying a template retunes that radio,
sets its sample rate, and adds channels and compatible outputs. The button names the target radio.
Templates outside its tuning or sample-rate capabilities are disabled.

Templates cover broadcast, aviation, marine, paging, amateur, digital voice, ISM, and other
services. Choose a setup for a signal available at your location.

Undo removes the added nodes but does not restore the previous radio frequency or sample rate.

## Presets

Save a preset after arranging and tuning a workspace. Applying it restores the graph and radio
settings using saved hardware identities. The apply report lists anything that could not be restored.

Presets are editable and stored on the server. Templates ship with the app and are read-only.

## Bookmarks and band plans

A bookmark saves the selected Device or decoder's frequency and tunes it back without changing
the graph.

**Bands** chooses the band-plan region and searches its allocations. A hit tunes the selected
Device, or the selected decoder — pulling its radio over when it cannot hear that frequency.
Enable the Scope allocation ruler to browse them: hover for details or click to tune, using the
usual mode when available.

Automatic region selection uses browser location and requires HTTPS or localhost. Manual selection
is always available.
