# Workspaces and presets

| | Holds | Use it to |
|---|---|---|
| Workspace | Nodes, wires, rack, radio settings, band-plan region | Keep a whole receiver |
| Template | A ready-made receiver, built in | Start a common setup |
| Preset | Settings and channels of every open radio | Get back to a known state |
| Bookmark | A frequency, a name, and an optional mode | Retune quickly |

## Workspaces

Create, switch, rename, duplicate, export, and delete workspaces from the name in the top bar. A
new database starts with a Device wired to a Scope, and a Speaker. Later workspaces start empty.
Changes save automatically.

Switching changes the active workspace for every client, including [phones](phones.md), which can
switch it too.

### Undo

Use the top-bar arrows, `Ctrl`/`⌘ Z`, and `Ctrl`/`⌘ Shift Z`. Undo changes the running receiver
for every client: undoing an added channel closes it, and undoing a dial move tunes back. Moves of
one control within a second count as one step. The server keeps 100 steps per workspace.

### Copy and paste

Select nodes, then `Ctrl`/`⌘ C` and `Ctrl`/`⌘ V`. Copies land beside the originals with the wires
between them. Pasted Device and Recording nodes need a radio or file picked. The clipboard works
across workspaces while the tab stays open.

### Export and import

The download button beside a workspace saves it as JSON, with its tuning. **Import a workspace
file** adds it as a new workspace and switches to it. Radios that are present open with the saved
settings. Missing ones stay disconnected and show *radio not connected*, so you can pick
replacements.

## Templates

Select a Device, or have only one, then open **Library → Templates** and press **Apply**. A
template retunes that radio, sets its rate, and adds channels and outputs. Templates the radio
cannot run are greyed out.

Undo removes the added nodes but leaves the radio's new frequency and rate.

## Presets

A preset saves every open radio as it is now: its settings and its channels. Applying it puts them
back on the matching radios open in the active workspace. It does not add or remove nodes.

## Bookmarks and band plans

**Bookmarks** saves the selected Device's or channel's frequency under a label and tunes it back.

**Bands** picks the workspace's band-plan region and searches its allocations. The default is ITU
Region 1. A hit tunes the selected Device, or the selected channel, moving its radio if needed.
**Ruler** in Bands, or **Band plan** in a Scope's settings, draws allocations on every Scope. Hover
one for details, click it to tune.
