# Presets, bookmarks and templates

Three different things, deliberately:

| | Scope | Source | Answers |
|---|---|---|---|
| **Bookmark** | one frequency | you | "where was that repeater?" |
| **Preset** | a whole device set | you, captured from a live set | "put my station back the way it was" |
| **Template** | a whole device set | shipped, read-only, device-agnostic | "I have never done this before — show me" |

All of it lives in the server's SQLite database, not in browser storage. Your station
configuration is part of your station, so every client sees the same setup. The browser keeps
only UI preferences like theme and layout.

## Bookmarks

A label, a frequency, an optional suggested channel type, and an optional group. Tune to one
with a click; use one as a scanner target list.

```http
GET    /api/bookmarks
POST   /api/bookmarks   {"label":"Local repeater","freq_hz":145600000,"mode":"nfm","group":"2m"}
DELETE /api/bookmarks/{id}
```

## Presets

A preset is a versioned snapshot of a live device set: the device it came from, its full
settings, and every channel's settings.

```http
GET    /api/presets
POST   /api/presets           {"name":"Airband bench"}   # snapshot a live set
POST   /api/presets/{id}/apply
DELETE /api/presets/{id}
```

Applying a preset retunes the device and replaces its channels. The apply is ordered
remove → patch → add, and validated against the live set rather than a stale snapshot; if it
cannot complete, the error says what state the set was left in instead of pretending success.

The snapshot carries a schema version so a stored preset from an older build is migrated or
rejected explicitly, never silently misread.

Because a preset names its device, it is the right tool for *your* station and the wrong tool
for sharing.

## Templates

A template is a read-only, shipped configuration that names **no device**: a centre
frequency, a sample rate, the channels to create, the tuning span it needs, a one-line
description and a short "what am I looking at" explainer. The same entry therefore applies to
whatever hardware you have open, and the gallery can mark entries your device cannot reach
instead of failing when you click them.

```http
GET  /api/templates
POST /api/templates/{id}/apply   {"device_set": 0}
```

The shipped set is FM radio (with RDS), airband, aircraft (ADS-B), ships (AIS), APRS, pagers
(POCSAG), ham 2 m and marine VHF. More entries follow their decoders — a template is only
worth shipping once the channels it creates exist.

Templates need no special engine code — they are presets plus a layout, and applying one goes
through the same path as anything else.

## Workspaces

Server-persisted workspaces and tabs (one active workspace, unlimited tabs, each a dockable
panel layout) are part of the UI shell described in `PLAN.md` §10. They are not built yet;
the current UI is a fixed panel layout that collapses to a single column below 768 px.
