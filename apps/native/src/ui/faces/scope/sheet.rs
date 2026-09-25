use zgui::prelude::*;

pub const SHEET: &str = css!(
    r#"
.scope { flex: 1 1 334px; min-height: 0; flex-direction: column; background-color: #000000; border-bottom-left-radius: 6px; border-bottom-right-radius: 6px; overflow: hidden; }
.scope__empty { flex: 1 1 auto; align-items: center; justify-content: center; }
.scope__plot { position: relative; flex: 1 1 auto; min-height: 0; overflow: hidden; background-color: #000000; cursor: default; }
.scope__plot.active { cursor: crosshair; }
.scope__plot.panning, .scope__plot.panning * { cursor: grabbing; }
.scope__trace { position: relative; flex: 0 0 auto; width: 100%; }
.scope__fall { position: relative; flex: 1 1 auto; min-height: 0; width: 100%; }
.scope__gpu { position: absolute; left: 0; top: 0; width: 100%; height: 100%; }
.scope__divider { position: relative; z-index: 10; flex: 0 0 auto; height: 9px; margin: -4px 0; cursor: row-resize; }
.scope__divider-line { position: absolute; left: 0; right: 0; top: 4px; height: 1px; background-color: rgba(127, 127, 127, 0.45); }
.scope__divider:hover .scope__divider-line, .scope__divider.held .scope__divider-line { background-color: var(--accent); }

.scope__labels { position: absolute; left: 0; top: 0; right: 0; bottom: 0; pointer-events: none; }
.scope__db { position: absolute; left: 4px; font-family: var(--mono); font-size: 10px; line-height: 12px; color: #7f7f7f; }
.scope__hz { position: absolute; bottom: 0; height: 16px; line-height: 16px; transform: translateX(-50%); font-family: var(--mono); font-size: 10px; color: #7f7f7f; white-space: nowrap; }
.scope__readout { position: absolute; top: 4px; height: 14px; line-height: 14px; padding: 0 4px; background-color: rgba(0, 0, 0, 0.85); font-family: var(--mono); font-size: 10px; color: #ffffff; white-space: pre; }

.scope__markers { position: absolute; left: 0; top: 0; right: 0; bottom: 0; pointer-events: none; overflow: hidden; }
.scope__shade { position: absolute; top: 0; bottom: 0; transform: translateX(-50%); background-color: rgba(255, 255, 255, 0.06); }
.scope__shade.on { background-color: rgba(255, 255, 255, 0.12); }
.scope__line { position: absolute; top: 0; bottom: 0; width: 1px; transform: translateX(-50%); background-color: #7f7f7f; }
.scope__line.on { width: 2px; background-color: #ffffff; }
.scope__grab { position: absolute; top: 0; bottom: 0; width: 24px; transform: translateX(-50%); pointer-events: auto; cursor: ew-resize; }
.scope__grab.held { cursor: pointer; }
.scope__chips { position: absolute; top: 28px; transform: translateX(-50%); align-items: center; gap: 4px; pointer-events: auto; }
.scope__chip { gap: 4px; padding: 0 4px; border: 1px solid var(--line); border-radius: 2px; background-color: rgba(13, 15, 16, 0.85); font-family: var(--mono); font-size: 10px; line-height: 14px; color: var(--ink-dim); white-space: nowrap; cursor: ew-resize; }
.scope__chip.held { cursor: pointer; }
.scope__chip.on { border-color: var(--accent); background-color: var(--bg); color: var(--accent); }
.scope__chip--member { display: none; }
.scope__chips:hover .scope__chip--member { display: flex; }
.scope__chips:hover .scope__chip--stack { display: none; }
.scope__count { color: #7f7f7f; }

.scope__bookmark { position: absolute; top: 0; bottom: 0; width: 0; border-left: 1px dashed color-mix(in oklab, var(--accent) 50%, transparent); }
.scope__tag { position: absolute; top: 3px; transform: translateX(-50%); align-items: center; pointer-events: auto; }
.scope__tag-label { gap: 4px; padding: 0 4px; border: 1px solid color-mix(in oklab, var(--accent) 40%, transparent); border-radius: 2px; background-color: rgba(13, 15, 16, 0.85); font-family: var(--mono); font-size: 10px; line-height: 14px; color: var(--accent); white-space: nowrap; }
.scope__tip { display: none; margin-top: 3px; padding: 4px 6px; border: 1px solid var(--line); border-radius: 4px; background-color: var(--panel); font-family: var(--mono); font-size: 10px; color: var(--ink-dim); white-space: nowrap; }
.scope__tag:hover .scope__tip { display: flex; }

.scope__chrome { position: absolute; left: 0; top: 0; right: 0; bottom: 0; pointer-events: none; }
.scope__legend { position: absolute; right: 6px; top: 6px; font-family: var(--mono); font-size: 10.5px; line-height: 14px; color: #7f7f7f; white-space: pre; }
.scope__plot.ruled .scope__legend { top: 22px; }
.scope__tools { position: absolute; left: 6px; bottom: 6px; align-items: center; gap: 4px; padding: 2px; border-radius: 3px; background-color: rgba(0, 0, 0, 0.85); pointer-events: auto; }
.scope__button { align-items: center; height: 20px; padding: 0 6px; border-radius: 3px; font-family: var(--mono); font-size: 10.5px; line-height: 20px; color: var(--ink-faint); }
.scope__button:hover { background-color: var(--panel-2); color: var(--ink); }
.scope__button.on { color: var(--accent); }
.scope__gear { width: 22px; padding: 0 5px; }
.scope__icon { width: 12px; height: 12px; }

.scope__panel { position: absolute; left: 6px; bottom: 34px; width: 280px; max-height: calc(100% - 44px); overflow: auto; gap: 12px; padding: 10px; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5); pointer-events: auto; cursor: default; }
.scope__section { gap: 6px; }
.scope__section-head { flex-direction: row; align-items: center; gap: 8px; }
.scope__section-name { font-family: var(--mono); font-size: 10px; letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-faint); }
.scope__swatches { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 6px; }
.scope__swatch { flex-direction: column; gap: 4px; padding: 4px; border: 1px solid transparent; border-radius: 3px; }
.scope__swatch:hover { background-color: var(--panel-2); }
.scope__swatch.on { border-color: var(--accent-dim); background-color: color-mix(in oklab, var(--accent) 10%, transparent); }
.scope__ramp { width: 100%; height: 12px; border-radius: 2px; }
.scope__swatch-name { font-family: var(--mono); font-size: 10.5px; color: var(--ink-faint); }
.scope__swatch.on .scope__swatch-name { color: var(--accent); }
.scope__toggles { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.scope__toggle { flex-direction: row; align-items: center; gap: 6px; padding: 3px 6px; border: 1px solid var(--line); border-radius: 3px; font-size: 11px; color: var(--ink-dim); }
.scope__toggle:hover { color: var(--ink); }
.scope__toggle.on { border-color: var(--accent-dim); background-color: color-mix(in oklab, var(--accent) 10%, transparent); color: var(--ink); }
.scope__sample-box { display: flex; align-items: center; flex: 0 0 auto; width: 20px; height: 14px; padding: 0 2px; border-radius: 3px; background-color: #000000; overflow: hidden; }
.scope__sample { width: 100%; height: 2px; border-radius: 999px; }
.scope__sample--peak { background-color: #ffff00; }
.scope__sample--average { background-color: #ffffff; }
.scope__sample--min { background-color: #7f7f7f; }
.scope__sample--phosphor { width: 100%; height: 100%; }
.scope__level { flex-direction: row; align-items: center; gap: 10px; }
.scope__level-name { flex: 0 0 auto; width: 44px; font-family: var(--mono); font-size: 10.5px; color: var(--ink-faint); }
.scope__faint { font-size: 11px; color: var(--ink-faint); }

.scope__menu { position: absolute; z-index: 30; width: 224px; transform: translateX(-50%); flex-direction: column; padding: 4px; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5); pointer-events: auto; cursor: default; }
.scope__menu-head { padding: 4px 8px; font-family: var(--mono); font-size: 12px; color: var(--ink); }
.scope__menu-row { padding: 4px 8px; border-radius: 4px; font-size: 12px; color: var(--ink-dim); }
.scope__menu-row:hover { background-color: var(--panel-2); color: var(--ink); }
.scope__menu-row.on { color: var(--accent); }
.scope__menu-row:disabled { color: var(--line-strong); }
.scope__form { gap: 4px; padding: 4px; }
.scope__field .native-input { height: 26px; width: 100%; padding: 3px 8px; font-family: var(--mono); font-size: 11px; background-color: var(--panel-2); color: var(--ink); border: 1px solid var(--line); border-radius: 5px; }

.scope__picker { position: absolute; z-index: 40; left: 50%; top: 8px; bottom: 8px; width: 240px; margin-left: -120px; flex-direction: column; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5); pointer-events: auto; cursor: default; }
.scope__picker-head { align-items: center; gap: 8px; padding: 6px 8px; border-bottom: 1px solid var(--line); font-size: 12px; color: var(--ink); }
.scope__picker-list { flex: 1 1 auto; min-height: 0; overflow: auto; padding: 4px; }
.scope__close { width: 16px; text-align: center; color: var(--ink-faint); }
.scope__close:hover { color: var(--ink); }

.scope__ruler { position: relative; flex: 0 0 16px; height: 16px; background-color: var(--bg); border-bottom: 1px solid color-mix(in oklab, var(--line) 60%, transparent); cursor: pointer; }
.scope__band { position: absolute; top: 0; bottom: 0; overflow: hidden; }
.scope__band-edge { position: absolute; left: 0; top: 0; bottom: 0; width: 1px; }
.scope__band-name { position: absolute; left: 4px; right: 4px; top: 0; bottom: 0; font-family: var(--mono); font-size: 10px; line-height: 16px; color: var(--ink); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.scope__band-tip { position: absolute; z-index: 30; top: 22px; width: 288px; gap: 6px; padding: 8px; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5); pointer-events: none; cursor: default; }
.scope__tip-head { flex-direction: row; align-items: baseline; gap: 8px; }
.scope__tip-hz { font-family: var(--mono); font-size: 13px; color: var(--ink); }
.scope__tip-band { gap: 2px; padding-top: 6px; border-top: 1px solid var(--line); }
.scope__tip-title { flex-direction: row; align-items: flex-start; gap: 6px; }
.scope__swatch-dot { flex: 0 0 auto; width: 8px; height: 8px; margin-top: 6px; border-radius: 1px; }
.scope__tip-name { flex: 1 1 auto; font-size: 13px; color: var(--ink); }
.scope__tip-official { font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.scope__tip-notes { max-height: 34px; overflow: hidden; font-size: 12px; color: var(--ink-dim); }
.scope__provisions { flex-direction: row; flex-wrap: wrap; gap: 4px; }
.scope__provision { padding: 0 4px; border: 1px solid var(--line); border-radius: 3px; font-family: var(--mono); font-size: 10px; color: var(--ink-faint); }
.scope__provision.known { color: var(--ink-dim); }
.band--ism { background-color: rgb(194, 115, 109); }
.band-fill--ism { background-color: rgba(194, 115, 109, 0.25); }
.band--broadcast { background-color: rgb(185, 125, 74); }
.band-fill--broadcast { background-color: rgba(185, 125, 74, 0.25); }
.band--mobile { background-color: rgb(105, 155, 106); }
.band-fill--mobile { background-color: rgba(105, 155, 106, 0.25); }
.band--science { background-color: rgb(82, 156, 137); }
.band-fill--science { background-color: rgba(82, 156, 137, 0.25); }
.band--maritime { background-color: rgb(58, 157, 162); }
.band-fill--maritime { background-color: rgba(58, 157, 162, 0.25); }
.band--aeronautical { background-color: rgb(71, 150, 192); }
.band-fill--aeronautical { background-color: rgba(71, 150, 192, 0.25); }
.band--navigation { background-color: rgb(113, 139, 195); }
.band-fill--navigation { background-color: rgba(113, 139, 195, 0.25); }
.band--amateur { background-color: rgb(149, 126, 192); }
.band-fill--amateur { background-color: rgba(149, 126, 192, 0.25); }
.band--satellite { background-color: rgb(183, 115, 154); }
.band-fill--satellite { background-color: rgba(183, 115, 154, 0.25); }
.band--other { background-color: rgb(135, 127, 115); }
.band-fill--other { background-color: rgba(135, 127, 115, 0.25); }
"#
);
