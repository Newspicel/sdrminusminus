# Workspaces and presets

| | Holds | Use it to |
|---|---|---|
| Workspace | Nodes, wires, rack, radio settings, band plan | Keep a whole receiver |
| Template | A ready-made receiver, built in | Start a common setup |
| Preset | A saved copy of a tuned workspace | Get back to a known state |
| Bookmark | A frequency and a name | Retune quickly |

## Workspaces

Create, switch, rename, and delete workspaces from the name in the top bar. A new database starts
with a Device, a Scope, and a Speaker. Later workspaces start empty. Changes save on their own.

### Undo

Use the top-bar arrows, `Ctrl`/`⌘ Z`, and `Ctrl`/`⌘ Shift Z`. Undo changes the running receiver
for every client: undoing an added channel closes it. The server keeps 100 steps per workspace.
Tuning is not part of the history.

### Copy and paste

Select nodes, then `Ctrl`/`⌘ C` and `Ctrl`/`⌘ V`. Copies land beside the originals with the wires
between them. Pasted Device nodes need a radio picked. The clipboard works across workspaces
while the tab stays open.

### Export and import

The ↓ button downloads the workspace as JSON. **Import a workspace file** adds it as a new
workspace. Radios that are present open with the saved settings. Missing ones stay disconnected
and are listed in the apply report, so you can pick replacements.

## Templates

Select a Device, then open **Library → Templates**. A template retunes that radio, sets its rate,
and adds channels and outputs. Templates the radio cannot handle are greyed out.

Undo removes the added nodes but leaves the radio's new frequency and rate.

## Presets

Save a preset once a workspace is set up and tuned. Applying it restores the nodes and the radio
settings. The apply report lists anything it could not restore.

## Bookmarks and band plans

A bookmark saves the selected Device's or channel's frequency and tunes it back.

**Bands** picks your band-plan region and searches its allocations. A hit tunes the selected
Device, or the selected channel, moving its radio if needed. Turn on the Scope's band ruler to
browse allocations: hover for details, click to tune.

The region is picked from your location when the page is on HTTPS or localhost. You can always
pick it by hand.
