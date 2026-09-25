use zgui::prelude::*;

pub const SHEET: &str = css!(
    r#"
.tools {
    position: fixed;
    left: 0;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 80;
    display: flex;
    align-items: center;
    justify-content: center;
    background-color: rgba(16, 17, 19, 0.7);
}

.tools__box {
    display: flex;
    flex-direction: column;
    width: 768px;
    max-width: 96%;
    max-height: 80%;
    border: 1px solid var(--line);
    border-radius: 10px;
    background-color: var(--panel);
    box-shadow: 0 22px 52px rgba(0, 0, 0, 0.62);
    overflow: hidden;
}

.tools__box.full { width: 97%; max-width: 97%; height: 94%; max-height: 94%; }

.tools__head { flex: 0 0 auto; align-items: baseline; gap: 16px; padding: 12px 16px; border-bottom: 1px solid var(--line); }
.tools__title { font-size: 14px; font-weight: 600; color: var(--ink); }
.tools__body { flex: 1 1 auto; min-height: 0; overflow: auto; padding: 16px; }
.tools__foot { flex: 0 0 auto; align-items: center; gap: 8px; padding: 10px 16px; border-top: 1px solid var(--line); }
.tools__list { flex-direction: column; border: 1px solid var(--line); border-radius: 6px; overflow: hidden; }
.tools__row { display: flex; flex-direction: column; padding: 7px 10px; border-bottom: 1px solid var(--line); font-size: 12px; }
.tools__row:hover { background-color: var(--panel-2); }

.tool-stack { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.tool-row { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 12px 14px; }
.tool-bar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.tool-box { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 8px; padding: 8px; border: 1px solid var(--line); border-radius: 4px; background-color: var(--panel-2); }
.tool-field { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.tool-field .pick__wrap { min-width: 150px; }
.tool-num { align-items: center; gap: 6px; }
.tool-num .entry { width: 96px; flex: 0 0 auto; }
.tool-num__unit { font-family: var(--mono); font-size: 11px; color: var(--ink-faint); }
.tool-text { width: 160px; }
.tool-text.wide { width: 256px; }
.tool-text.narrow { width: 104px; }
.tool-text .native-input {
    width: 100%;
    padding: 3px 8px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background-color: var(--panel-2);
    color: var(--ink);
    font-size: 12px;
}

.tool-chips { display: flex; flex-wrap: wrap; gap: 8px; }
.tool-chip {
    align-items: baseline;
    gap: 6px;
    padding: 2px 8px;
    border: 1px solid var(--line);
    border-radius: 999px;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink);
}
.tool-chip__key { color: var(--ink-faint); }
.tool-chip.on { border-color: var(--accent); color: var(--accent); }
.tool-chip:hover { border-color: var(--line-strong); }

.tool-alert { padding: 6px 10px; border: 1px solid var(--danger); border-radius: 4px; color: var(--danger); font-size: 12px; }
.tool-ink { color: var(--ink); }
.tool-dim { color: var(--ink-dim); font-size: 12px; }
.tool-faint { color: var(--ink-faint); font-size: 11px; }
.tool-accent { color: var(--accent); }
.tool-ok { color: var(--ok); }
.tool-danger { color: var(--danger); }
.tool-warn { color: oklch(0.8 0.14 80); }
.tool-sans { font-family: system-ui, sans-serif; }
.tool-mono { font-family: var(--mono); font-size: 12px; }
.tool-mono.ok { color: var(--ok); }

.btn.primary { border-color: var(--accent-dim); color: var(--accent); }
.btn.danger { border-color: var(--danger); color: var(--danger); }
.btn.small { padding: 1px 8px; font-size: 10px; }
.btn:disabled { color: var(--line-strong); border-color: var(--line); }

.tool-notes { display: flex; flex-direction: column; gap: 6px; }
.tool-note { gap: 8px; font-size: 12px; color: var(--ink-dim); }

.tool-group { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.tool-group__lines { display: flex; flex-direction: column; gap: 2px; }
.tool-groups { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px 24px; }
.tool-line { align-items: baseline; justify-content: space-between; gap: 12px; padding: 2px 0; border-bottom: 1px solid var(--line); }
.tool-line__key { flex: 0 0 auto; font-size: 12px; color: var(--ink-dim); }
.tool-line__value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--mono); font-size: 12px; color: var(--ink); }
.tool-line__value.accent { color: var(--accent); }

.tool-table { display: grid; column-gap: 0; font-family: var(--mono); font-size: 12px; }
.tool-th { position: sticky; top: 0; padding: 4px 8px; background-color: var(--panel-3); font-size: 10.5px; color: var(--ink-faint); }
.tool-td { padding: 4px 8px; border-top: 1px solid var(--line); color: var(--ink); }
.tool-td.lit { background-color: var(--panel-2); }
.tool-td.tool-dim, .tool-td.dim { color: var(--ink-dim); }
.tool-td.faint { color: var(--ink-faint); }
.tool-td.tool-accent { color: var(--accent); }
.tool-scroll { min-height: 0; flex: 1 1 auto; overflow: auto; border: 1px solid var(--line); border-radius: 4px; }
.tool-pre { padding: 8px; border: 1px solid var(--line); border-radius: 4px; background-color: var(--panel-2); font-family: var(--mono); font-size: 11px; color: var(--ink-dim); white-space: pre; overflow: auto; }

.ant-parts { grid-template-columns: minmax(0, 1fr) auto auto auto; }
.ant-view { display: flex; flex-direction: column; gap: 8px; }
.ant-view__head { align-items: center; gap: 10px; }
.ant-view__hover { font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.ant-draw { position: relative; width: 640px; height: 320px; flex: 0 0 auto; border: 1px solid var(--line); border-radius: 4px; background-color: var(--panel-2); overflow: hidden; }
.ant-draw.orbit { cursor: grab; }
.ant-draw__canvas { position: absolute; left: 0; top: 0; width: 640px; height: 320px; }
.ant-label { position: absolute; font-family: var(--mono); font-size: 10px; color: var(--ink-dim); white-space: nowrap; pointer-events: none; }
.ant-label.centre { transform: translateX(-50%); }
.ant-label.up { transform: translate(-50%, -50%) rotate(-90deg); }
.ant-legend { display: flex; flex-wrap: wrap; gap: 4px 16px; }
.ant-legend__item { align-items: center; gap: 6px; font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }
.ant-legend__swatch { width: 16px; height: 4px; }

.cps { display: flex; flex-direction: column; gap: 12px; height: 100%; min-height: 0; }
.cps__job { align-items: center; gap: 12px; padding: 6px 8px; border: 1px solid var(--line); border-radius: 4px; background-color: var(--panel-2); }
.cps__job-text { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--mono); font-size: 12px; }
.cps__progress { width: 160px; height: 4px; border-radius: 999px; background-color: var(--panel-3); overflow: hidden; }
.cps__progress-fill { height: 4px; background-color: var(--accent); }
.cps__main { display: flex; flex: 1 1 auto; min-height: 0; gap: 12px; }
.cps__side { display: flex; flex-direction: column; gap: 16px; width: 288px; flex: 0 0 auto; overflow: auto; padding-right: 4px; }
.cps__work { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; gap: 8px; }
.cps__item { align-items: baseline; justify-content: space-between; gap: 8px; padding: 4px; border-top: 1px solid var(--line); border-radius: 3px; font-size: 12px; }
.cps__item.on { background-color: var(--panel-2); }
.cps__pick { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; }
.cps__report { display: flex; flex-direction: column; gap: 8px; padding: 12px; border: 1px solid var(--line); border-radius: 4px; background-color: var(--panel-2); }
.cps__channels { grid-template-columns: auto minmax(0, 1fr) auto auto auto auto minmax(0, 1.4fr) auto; }
.cps__two { grid-template-columns: auto minmax(0, 1fr); }
.cps__three { grid-template-columns: minmax(0, 1fr) auto auto; }

.vna-chart { position: relative; height: 300px; flex: 0 0 auto; border: 1px solid var(--line); border-radius: 4px; background-color: #000000; overflow: hidden; }
.vna-chart:focus-visible { border-color: var(--accent); outline: none; }
.vna-chart__canvas { position: absolute; left: 0; top: 0; width: 100%; height: 300px; }
.vna-plot { position: absolute; left: 64px; right: 18px; top: 16px; bottom: 38px; pointer-events: none; }
.vna-label { position: absolute; font-family: var(--mono); font-size: 10px; color: #7f7f7f; white-space: nowrap; pointer-events: none; }
.vna-label.hold { color: #ffff00; }
.vna-label.trace { color: #66e5ff; }
.vna-tick { position: absolute; left: 0; width: 56px; text-align: right; }
.vna-title { text-align: center; }
.vna-smith { position: relative; width: 340px; height: 340px; flex: 0 0 auto; align-self: center; border: 1px solid var(--line); border-radius: 4px; background-color: #000000; overflow: hidden; }
.vna-smith__canvas { position: absolute; left: 0; top: 0; width: 340px; height: 340px; }
.vna-marker { align-items: center; gap: 8px; }
.vna-marker__at { width: 110px; text-align: right; font-family: var(--mono); font-size: 12px; color: var(--ink); }
.vna-step { align-items: center; gap: 8px; }
.vna-step .btn { width: 112px; text-align: center; }
.vna-export { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 8px; padding-top: 12px; border-top: 1px solid var(--line); }
"#
);
