use zgui::prelude::*;

pub const SHEET: &str = css!(
    r#"
:root {
    --bg: oklch(0.178 0.004 255);
    --panel: oklch(0.215 0.005 255);
    --panel-2: oklch(0.255 0.006 255);
    --panel-3: oklch(0.3 0.007 255);
    --line: oklch(0.33 0.007 255);
    --line-strong: oklch(0.5 0.009 255);
    --ink: oklch(0.95 0.003 255);
    --ink-dim: oklch(0.74 0.006 255);
    --ink-faint: oklch(0.62 0.008 255);
    --accent: oklch(0.74 0.13 258);
    --accent-dim: oklch(0.58 0.115 258);
    --danger: oklch(0.7 0.17 25);
    --ok: oklch(0.78 0.15 155);

    --plot-ink-dim: #7f7f7f;

    --mono: ui-monospace, "SF Mono", Menlo, monospace;

    display: flex;
    flex-direction: column;
    background-color: var(--bg);
    color: var(--ink);
    font-family: system-ui, sans-serif;
    font-size: 13px;
    line-height: 1.5;
    overflow: hidden;
}

.legend {
    font-family: var(--mono);
    font-size: 10px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ink-faint);
}

.mono { font-family: var(--mono); }

.shell { flex: 1 1 auto; min-height: 0; }

.head {
    flex: 0 0 auto;
    align-items: center;
    gap: 2px;
    height: 44px;
    padding: 0 14px;
    border-bottom: 1px solid var(--line);
    background-color: var(--bg);
}

.head__mark {
    font-family: var(--mono);
    font-size: 15px;
    font-weight: 700;
    letter-spacing: 0.02em;
    color: var(--ink);
    padding-right: 18px;
}

.head__name { color: var(--ink-dim); padding: 0 18px 0 0; }

.head__rule { width: 1px; height: 20px; margin: 0 10px; background-color: var(--line); }

.tab {
    padding: 4px 14px;
    border-radius: 7px;
    color: var(--ink-dim);
    font-size: 13px;
}

.tab:hover { background-color: var(--panel-2); color: var(--ink); }

.tab.on {
    background-color: color-mix(in oklab, var(--accent) 20%, transparent);
    color: var(--accent);
}

.head__link { padding: 4px 12px; border-radius: 7px; color: var(--ink-dim); }
.head__link:hover { background-color: var(--panel-2); color: var(--ink); }

.head__step {
    width: 28px;
    height: 26px;
    border-radius: 7px;
    text-align: center;
    line-height: 25px;
    font-size: 14px;
    color: var(--ink-dim);
}

.head__step:hover { background-color: var(--panel-2); color: var(--ink); }
.head__step:disabled { color: var(--line); }

.pane { flex: 1 1 auto; min-height: 0; position: relative; overflow: hidden; display: flex; }

.patch { flex: 1 1 auto; min-width: 0; position: relative; overflow: hidden; display: flex; }


.node {
    width: 100%;
    height: 100%;
    border: 1px solid var(--line);
    border-radius: 7px;
    background-color: var(--panel);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.45), 0 10px 28px rgba(0, 0, 0, 0.42);
}

.flow__node.selected .node {
    z-index: 5;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px color-mix(in oklab, var(--accent) 45%, transparent),
        0 1px 2px rgba(0, 0, 0, 0.45), 0 10px 28px rgba(0, 0, 0, 0.42);
}

.node__bar {
    align-items: center;
    gap: 10px;
    height: 26px;
    padding: 0 9px;
    border-bottom: 1px solid var(--line);
    border-top-left-radius: 6px;
    border-top-right-radius: 6px;
    background-color: var(--panel-2);
}

.node__title {
    font-family: var(--mono);
    font-size: 10px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--ink-dim);
    overflow: hidden;
}

.node__state { font-family: var(--mono); font-size: 9px; letter-spacing: 0.09em; }
.node__state.run { color: var(--ok); }
.node__state.err { color: var(--danger); }
.node__state.idle { color: var(--ink-faint); }

.node__shut {
    width: 15px;
    height: 15px;
    border-radius: 4px;
    color: var(--ink-faint);
    text-align: center;
    line-height: 14px;
    font-size: 11px;
}

.node__shut:hover { background-color: var(--panel-3); color: var(--danger); }

.face { flex-direction: column; gap: 9px; padding: 11px 12px 12px 12px; }

.face__foot {
    align-items: center;
    margin: 2px -12px -12px -12px;
    padding: 6px 12px;
    border-top: 1px solid var(--line);
}

.flow .flow__handle-dot {
    width: 10px;
    height: 10px;
    border: 2px solid var(--bg);
    background-color: var(--line-strong);
}

.flow__handle[data-class="iq"] .flow__handle-dot { background-color: oklch(0.72 0.11 228); }
.flow__handle[data-class="baseband"] .flow__handle-dot { background-color: oklch(0.74 0.1 196); }
.flow__handle[data-class="audio"] .flow__handle-dot { background-color: oklch(0.74 0.12 158); }
.flow__handle[data-class="events"] .flow__handle-dot { background-color: oklch(0.78 0.11 85); }
.flow__handle[data-class="video"] .flow__handle-dot { background-color: oklch(0.76 0.12 35); }
.flow__handle[data-class="control"] .flow__handle-dot { background-color: oklch(0.76 0.1 300); }
.flow__handle[data-class="position"] .flow__handle-dot { background-color: oklch(0.74 0.09 140); }
.flow__handle[data-class="tx"] .flow__handle-dot { background-color: oklch(0.76 0.12 345); }

.flow__handle:hover .flow__handle-dot { border-color: var(--ink); }
.flow__handle[data-status="from"] .flow__handle-dot { border-color: var(--ink); }
.flow__handle[data-status="valid"] .flow__handle-dot { border-color: var(--ok); }
.flow__handle[data-status="invalid"] .flow__handle-dot { border-color: var(--danger); }

.flow .flow__handle-label {
    top: 0;
    padding: 0 4px;
    border-radius: 3px;
    background-color: color-mix(in oklab, var(--bg) 85%, transparent);
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
}

.flow .flow__handle-label[data-side="left"] { left: auto; right: 18px; }
.flow .flow__handle-label[data-side="right"] { right: auto; left: 18px; }

.flow__minimap, .flow__controls { border-color: var(--line); background-color: var(--panel); }
.flow__control { color: var(--ink-dim); }
.flow__control:hover { background-color: var(--panel-2); color: var(--ink); }
.flow__selection { border-color: var(--accent); background-color: color-mix(in oklab, var(--accent) 12%, transparent); }

.dial { align-items: center; gap: 0; flex: 0 0 auto; }

.digit {
    display: block;
    flex: 0 0 auto;
    font-family: var(--mono);
    font-size: 23px;
    line-height: 1.15;
    letter-spacing: 0.02em;
    color: var(--ink);
    width: 15px;
    text-align: center;
    border-bottom: 2px solid transparent;
}

.digit.dot { width: 8px; color: var(--ink); }

.digit.on { border-bottom-color: var(--accent); }
.digit:hover { color: var(--accent); }
.digit.dim { color: var(--ink-faint); }

.dial__unit { font-family: var(--mono); font-size: 11px; color: var(--ink-faint); padding-left: 7px; }

.field { align-items: center; gap: 10px; min-height: 24px; }

.field__name {
    flex: 0 0 auto;
    width: 84px;
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ink-faint);
}

.field__body { flex: 1 1 auto; align-items: center; gap: 8px; min-width: 0; }

.rule { height: 1px; margin: 3px 0; background-color: var(--line); }

.section { align-items: center; gap: 10px; }

.pick__wrap { position: relative; flex: 1 1 auto; min-width: 0; }

.pick {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 3px 9px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background-color: var(--panel-2);
    color: var(--ink);
    font-size: 12px;
    min-width: 0;
}

.pick:hover { border-color: var(--line-strong); }
.pick.on { border-color: var(--accent-dim); }
.pick__caret { color: var(--ink-faint); font-size: 8px; }

.menu {
    position: absolute;
    left: 0;
    top: 26px;
    z-index: 60;
    flex-direction: column;
    min-width: 100%;
    max-height: 240px;
    overflow: auto;
    padding: 4px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background-color: var(--panel-2);
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
}

.menu__row { padding: 4px 8px; border-radius: 5px; color: var(--ink-dim); font-size: 12px; }
.menu__row:hover { background-color: var(--panel-3); color: var(--ink); }
.menu__row.on { color: var(--accent); }

.seg {
    align-items: center;
    gap: 2px;
    padding: 2px;
    border-radius: 7px;
    background-color: var(--panel-2);
    flex: 0 0 auto;
}

.seg__item { padding: 2px 11px; border-radius: 5px; color: var(--ink-dim); font-size: 11px; }
.seg__item:hover { color: var(--ink); }
.seg__item.on { background-color: var(--panel-3); color: var(--ink); }

.slide { flex: 1 1 auto; height: 16px; position: relative; align-items: center; min-width: 60px; }

.slide__rail { position: absolute; left: 0; right: 0; top: 7px; height: 3px; border-radius: 999px; background-color: var(--panel-3); }
.slide__fill { position: absolute; left: 0; top: 7px; height: 3px; border-radius: 999px; background-color: var(--accent-dim); }

.slide__grip {
    position: absolute;
    top: 3px;
    width: 11px;
    height: 11px;
    margin-left: -5px;
    border-radius: 999px;
    background-color: var(--accent);
}

.slide__read {
    flex: 0 0 auto;
    width: 56px;
    text-align: right;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-faint);
}

.check {
    width: 14px;
    height: 14px;
    flex: 0 0 auto;
    border: 1px solid var(--line-strong);
    border-radius: 4px;
    background-color: var(--panel-2);
    text-align: center;
    line-height: 12px;
    font-size: 9px;
    color: transparent;
}

.check.on { background-color: var(--accent); border-color: var(--accent); color: var(--bg); }

.btn {
    padding: 3px 11px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background-color: var(--panel-2);
    color: var(--ink-dim);
    font-size: 11px;
    flex: 0 0 auto;
}

.btn:hover { background-color: var(--panel-3); color: var(--ink); }

.bar { flex: 1 1 auto; align-items: center; gap: 10px; }
.meter { flex: 1 1 auto; height: 6px; border-radius: 2px; background-color: var(--panel-3); overflow: hidden; }
.meter__fill { height: 6px; background-color: var(--accent-dim); }

.meter__read {
    flex: 0 0 auto;
    width: 58px;
    text-align: right;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-faint);
}

.log { flex-direction: column; gap: 3px; height: 168px; overflow: auto; }
.log__row { gap: 9px; font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }
.log__when { flex: 0 0 auto; color: var(--ink-faint); }
.log__what { flex: 1 1 auto; overflow: hidden; }

.params { gap: 4px; }
.entry { flex: 1 1 auto; min-width: 0; }
.entry .native-input, .pal__search .native-input {
    height: 26px; min-width: 0; width: 100%; padding: 3px 8px;
    font-family: var(--mono); font-size: 11px; line-height: 18px;
    background-color: var(--panel-2); color: var(--ink);
    border: 1px solid var(--line); border-radius: 5px;
}
.entry .native-input:focus-visible, .pal__search .native-input:focus-visible { border-color: var(--accent); outline: 1px solid var(--accent); }
.entry .native-input:invalid { border-color: var(--danger); }
.params__row { flex-direction: column; }
.audio-controls { gap: 9px; }
.field__error { color: #f87171; font-size: 11px; }
.hint { color: var(--ink-faint); font-size: 11px; }

.rack { flex-direction: row; flex-wrap: wrap; align-content: flex-start; gap: 14px; padding: 14px; overflow: auto; }
.rack .node { position: relative; left: 0; top: 0; height: auto; }

.toast {
    position: absolute;
    left: 50%;
    bottom: 18px;
    margin-left: -200px;
    width: 400px;
    padding: 9px 13px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background-color: var(--panel-2);
    color: var(--ink);
    font-size: 12px;
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
}

.pal {
    position: absolute;
    left: 50%;
    top: 24px;
    margin-left: -230px;
    width: 460px;
    max-height: 460px;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: 10px;
    background-color: var(--panel);
    box-shadow: 0 22px 52px rgba(0, 0, 0, 0.62);
    overflow: hidden;
    z-index: 60;
}

.pal__head { align-items: center; padding: 9px 13px; border-bottom: 1px solid var(--line); background-color: var(--panel-2); }
.pal__search { margin: 8px; }
.pal > .seg { margin: 0 8px 8px; }
.pal__row:focus-visible { outline: 2px solid var(--accent); }
.pal__list { flex-direction: column; padding: 6px; overflow: auto; }

.pal__row { display: flex; width: 100%; gap: 12px; align-items: center; justify-content: space-between; padding: 6px 9px; border-radius: 6px; color: var(--ink-dim); }
.pal__row:hover { background-color: var(--panel-2); color: var(--ink); }
.pal__cat { font-family: var(--mono); font-size: 9px; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ink-faint); }
"#
);
