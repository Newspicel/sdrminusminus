use zgui::prelude::*;

pub const SHEET: &str = css!(
    r#"
.kit-icon { width: 16px; height: 16px; flex: 0 0 auto; }

.kit-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border: 1px solid var(--line);
    border-radius: 3px;
    background-color: var(--panel-2);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    flex: 0 0 auto;
}

.kit-btn:hover { border-color: var(--line-strong); background-color: var(--panel-3); }
.kit-btn:disabled { opacity: 0.45; }

.kit-btn--primary { border-color: var(--accent); background-color: var(--accent); color: var(--bg); font-weight: 600; }
.kit-btn--primary:hover { border-color: var(--accent); background-color: var(--accent); }

.kit-btn--quiet { border-color: transparent; background-color: transparent; color: var(--ink-dim); }
.kit-btn--quiet:hover { border-color: transparent; background-color: var(--panel-2); color: var(--ink); }

.kit-btn--danger:hover { border-color: var(--danger); color: var(--danger); background-color: color-mix(in oklab, var(--danger) 10%, transparent); }

.kit-icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid transparent;
    border-radius: 3px;
    color: var(--ink-dim);
    flex: 0 0 auto;
}

.kit-icon-btn:hover { background-color: var(--panel-2); color: var(--ink); }
.kit-icon-btn.on { background-color: color-mix(in oklab, var(--accent) 15%, transparent); color: var(--accent); }
.kit-icon-btn:disabled { opacity: 0.45; }

.kit-foot { gap: 6px; justify-content: flex-end; }

.kit-field { position: relative; flex: 1 1 auto; min-width: 0; align-items: center; gap: 6px; }
.kit-field .native-input {
    height: 28px; min-width: 0; width: 100%; padding: 3px 8px;
    font-family: var(--mono); font-size: 12px; line-height: 20px;
    background-color: var(--bg); color: var(--ink);
    border: 1px solid var(--line); border-radius: 3px;
}
.kit-field .native-input:hover { border-color: var(--line-strong); }
.kit-field .native-input:focus-visible { border-color: var(--accent-dim); outline: 2px solid color-mix(in oklab, var(--accent) 20%, transparent); }
.kit-field .native-input:invalid { border-color: var(--danger); }
.kit-field .native-input:disabled { opacity: 0.45; }
.kit-field__unit { flex: 0 0 auto; font-family: var(--mono); font-size: 11px; color: var(--ink-faint); }

.kit-readout { gap: 4px; padding-top: 8px; border-top: 1px solid var(--line); }
.kit-read { align-items: baseline; gap: 12px; }
.kit-read__name {
    flex: 0 0 auto;
    width: 84px;
    font-family: var(--mono);
    font-size: 10px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ink-faint);
}
.kit-read__value { flex: 1 1 auto; min-width: 0; font-family: var(--mono); font-size: 12px; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.kit-group { gap: 8px; padding-top: 8px; border-top: 1px solid var(--line); }
.kit-group__name {
    font-family: var(--mono);
    font-size: 10px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ink-faint);
}

.kit-fold { gap: 6px; }
.kit-fold__link { align-self: flex-start; padding: 2px 8px; border-radius: 3px; color: var(--ink-dim); font-size: 12px; }
.kit-fold__link:hover { background-color: var(--panel-2); color: var(--ink); }
.kit-fold__alert { color: var(--danger); font-size: 12px; }

.kit-note { color: var(--ink-dim); font-size: 12px; }
.kit-alert { color: var(--danger); font-family: var(--mono); font-size: 11px; }
.kit-mono { font-family: var(--mono); font-size: 12px; color: var(--ink); }
.kit-meta { font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }

.kit-dial-wrap { flex: 0 0 auto; }

.kit-dial {
    align-items: center;
    padding: 2px 6px;
    border: 1px solid var(--line);
    border-radius: 3px;
    background-color: var(--bg);
    font-family: var(--mono);
    line-height: 1;
    user-select: none;
}

.kit-dial:focus-visible { outline: 2px solid color-mix(in oklab, var(--accent) 45%, transparent); }
.kit-dial.off { opacity: 0.55; }

.kit-digit {
    position: relative;
    flex: 0 0 auto;
    min-height: 28px;
    padding: 0 2px;
    border-radius: 2px;
    overflow: hidden;
    font-size: 23px;
    line-height: 28px;
    color: oklch(0.86 0.1 85);
}

.kit-digit.dim { color: color-mix(in oklab, var(--ink-faint) 60%, transparent); }
.kit-digit.on, .kit-digit.up, .kit-digit.down { color: var(--accent); }
.kit-digit.on { background-color: color-mix(in oklab, var(--accent) 12%, transparent); border-bottom: 2px solid var(--accent); }
.kit-digit.up { cursor: n-resize; }
.kit-digit.down { cursor: s-resize; }
.kit-dial.off .kit-digit { cursor: default; }

.kit-digit__half { position: absolute; left: 0; right: 0; height: 50%; pointer-events: none; }
.kit-digit.up .kit-digit__half { top: 0; background-color: color-mix(in oklab, var(--accent) 18%, transparent); }
.kit-digit.down .kit-digit__half { bottom: 0; background-color: color-mix(in oklab, var(--accent) 18%, transparent); }
.kit-digit__glyph { position: relative; }

.kit-digit__sep { flex: 0 0 auto; font-size: 23px; line-height: 28px; color: color-mix(in oklab, oklch(0.86 0.1 85) 70%, transparent); min-width: 4px; }

.kit-dial__unit { align-self: center; padding-left: 8px; font-size: 11px; letter-spacing: 0.04em; color: var(--ink-faint); }

.kit-entry .native-input {
    height: 36px; width: 170px; padding: 3px 8px;
    font-family: var(--mono); font-size: 18px;
    background-color: var(--bg); color: var(--ink);
    border: 1px solid var(--accent); border-radius: 3px;
}
.kit-entry .native-input:invalid { border-color: var(--danger); }

.kit-pop-wrap { position: relative; flex: 0 0 auto; }

.kit-pop {
    position: absolute;
    right: 0;
    top: 32px;
    z-index: 70;
    width: 256px;
    gap: 8px;
    padding: 10px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background-color: var(--panel-3);
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
}

.kit-pop__row { align-items: center; gap: 8px; }
.kit-pop__hint { text-transform: none; letter-spacing: 0.02em; }

.kit-tabs { align-self: stretch; }
.kit-tabs .seg__item { flex: 1 1 0; text-align: center; }

.kit-list { flex-direction: column; gap: 4px; max-height: 256px; overflow: auto; }
.kit-choice {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 6px 10px;
    border: 1px solid var(--line);
    border-radius: 3px;
    background-color: var(--panel-2);
    color: var(--ink);
    font-size: 12px;
    flex: 0 0 auto;
}
.kit-choice:hover { border-color: var(--line-strong); background-color: var(--panel-3); }
.kit-choice:disabled { opacity: 0.45; }
.kit-choice__meta { font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }

.kit-muted { opacity: 0.45; pointer-events: none; }

.kit-head { align-items: center; gap: 8px; min-height: 18px; }
.kit-head__title { font-family: var(--mono); font-size: 11px; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kit-heard { font-family: var(--mono); font-size: 11px; }
.kit-heard[data-tone="ok"] { color: var(--ok); }
.kit-heard[data-tone="warn"] { color: oklch(0.8 0.14 85); }
.kit-heard[data-tone="danger"] { color: var(--danger); }

.kit-tuner { gap: 10px; padding-bottom: 10px; border-bottom: 1px solid var(--line); }
.kit-lane-block { position: relative; gap: 8px; }
.kit-lane-block.locked { border-left: 2px solid var(--accent); margin-left: -8px; padding-left: 6px; }
.kit-dial-row { align-items: center; gap: 8px; min-width: 0; }
.kit-tools { align-items: center; gap: 4px; flex: 0 0 auto; }

.kit-rule { align-items: center; gap: 8px; font-family: var(--mono); font-size: 10px; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ink-faint); }
.kit-rule__tick { width: 12px; height: 1px; background-color: var(--line); }
.kit-rule__line { flex: 1 1 auto; height: 1px; background-color: var(--line); }
.kit-rule__port { color: oklch(0.72 0.11 228); text-transform: none; }
.kit-rule__badge { align-items: center; gap: 4px; color: oklch(0.72 0.11 228); }
.kit-rule .kit-icon { width: 12px; height: 12px; }

.kit-radio { gap: 9px; }
.kit-lane { gap: 9px; }
.kit-gain { display: flex; flex-direction: column; }
.kit-slot { display: flex; flex: 1 1 auto; min-width: 0; align-items: center; }
.kit-auto { align-items: center; gap: 6px; flex: 0 0 auto; }

.kit-transport { align-items: center; gap: 6px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }

.kit-choices { gap: 8px; }
.kit-list-wrap { gap: 6px; }
.kit-list-wrap .kit-btn { justify-content: center; }
.kit-doctor { gap: 8px; }
.kit-check { gap: 2px; }
.kit-check__head { align-items: center; gap: 8px; }
.kit-check__status { font-family: var(--mono); font-size: 11px; color: var(--ok); }
.kit-check__status[data-status="warn"] { color: var(--ink); }
.kit-check__status[data-status="fail"] { color: var(--danger); }
.kit-check__detail { padding-left: 16px; font-family: var(--mono); font-size: 10px; color: var(--ink-faint); white-space: pre-wrap; }

.kit-alert-block { padding-top: 8px; border-top: 1px solid var(--line); }
.kit-read-out { flex: 0 0 auto; width: 56px; text-align: right; font-family: var(--mono); font-size: 12px; color: var(--ink); }
.kit-legend { font-family: var(--mono); font-size: 10px; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ink-faint); }
.kit-legend.warn { color: oklch(0.8 0.14 85); }
"#
);
