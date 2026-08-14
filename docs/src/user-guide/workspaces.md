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

## Templates

Open **Library → Templates** after selecting a Device node. A template retunes that radio, sets an
appropriate sample rate, adds its channels, and merges the necessary displays or outputs into the
workspace.

Built-in templates cover FM broadcast, civil airband, ADS-B, AIS, APRS, POCSAG, 2 m amateur radio,
and marine VHF. The server disables templates that the selected radio cannot tune or sample
correctly.

Applying a template changes live device and channel configuration immediately. The button names
the target radio because this operation is not an undoable preview.

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
