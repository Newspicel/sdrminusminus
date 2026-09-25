use sdrmm_wire::tools::{NanoVnaCalStep, NanoVnaCalibration, NanoVnaStandard, NanoVnaSweepState};
use zgui::prelude::*;

use super::rf::{calibrate_request, calibration_of};
use crate::{
    store::Store,
    ui::{
        tools::kit::{Query, alert, button, chip, format_hz, labelled, run_tool},
        widgets::pick,
    },
};

struct Step {
    standard: NanoVnaStandard,
    step: NanoVnaCalStep,
    key: &'static str,
    label: &'static str,
    hint: &'static str,
}

const ONE_PORT: [Step; 3] = [
    Step {
        standard: NanoVnaStandard::Open,
        step: NanoVnaCalStep::Open,
        key: "open",
        label: "Open",
        hint: "Leave CH0 open, or fit the OPEN standard.",
    },
    Step {
        standard: NanoVnaStandard::Short,
        step: NanoVnaCalStep::Short,
        key: "short",
        label: "Short",
        hint: "Fit the SHORT standard to CH0.",
    },
    Step {
        standard: NanoVnaStandard::Load,
        step: NanoVnaCalStep::Load,
        key: "load",
        label: "Load",
        hint: "Fit the 50 \u{3a9} LOAD standard to CH0.",
    },
];

const TWO_PORT: [Step; 2] = [
    Step {
        standard: NanoVnaStandard::Isolation,
        step: NanoVnaCalStep::Isolation,
        key: "isolation",
        label: "Isolation",
        hint: "Terminate CH1 in 50 \u{3a9}, with CH0 loaded.",
    },
    Step {
        standard: NanoVnaStandard::Thru,
        step: NanoVnaCalStep::Thru,
        key: "thru",
        label: "Thru",
        hint: "Join CH0 to CH1 with the THRU adapter.",
    },
];

#[must_use]
pub fn reflection_done(state: Option<&NanoVnaCalibration>) -> bool {
    ONE_PORT
        .iter()
        .all(|entry| state.is_some_and(|state| state.standards.contains(&entry.standard)))
}

#[must_use]
pub fn standard_name(standard: NanoVnaStandard) -> &'static str {
    match standard {
        NanoVnaStandard::Load => "load",
        NanoVnaStandard::Open => "open",
        NanoVnaStandard::Short => "short",
        NanoVnaStandard::Thru => "thru",
        NanoVnaStandard::Isolation => "isolation",
    }
}

#[derive(Clone, Copy)]
pub struct Calibrate {
    pub store: Store,
    pub port: Memo<String>,
    pub range: Memo<NanoVnaSweepState>,
    pub state: RwSignal<Option<NanoVnaCalibration>>,
}

#[derive(Clone, Copy)]
struct Runner {
    calibrate: Calibrate,
    pending: RwSignal<Option<&'static str>>,
    call: Query<()>,
}

impl Runner {
    fn run(self, key: &'static str, step: NanoVnaCalStep) {
        let Calibrate {
            store,
            port,
            range,
            state,
        } = self.calibrate;
        let range = matches!(step, NanoVnaCalStep::Reset).then(|| range.get_untracked());
        let request = calibrate_request(&port.get_untracked(), step, range);
        self.pending.set(Some(key));
        let pending = self.pending;
        self.call.run(async move {
            let response = run_tool(store, request).await;
            pending.try_set(None);
            let response = response?;
            if let Some(next) = calibration_of(Some(&response)) {
                state.try_set(Some(next.clone()));
            }
            Ok(())
        });
    }

    fn label(
        self,
        key: &'static str,
        idle: &'static str,
        busy: &'static str,
    ) -> impl Fn() -> String + 'static {
        let pending = self.pending;
        move || {
            if pending.get() == Some(key) {
                busy.to_owned()
            } else {
                idle.to_owned()
            }
        }
    }

    fn busy(self) -> impl Fn() -> bool + Send + Sync + 'static {
        let call = self.call;
        move || call.busy.get()
    }
}

pub fn panel(calibrate: Calibrate) -> impl IntoView {
    let runner = Runner {
        calibrate,
        pending: RwSignal::new(None),
        call: Query::new(),
    };
    let slot = RwSignal::new(0u8);
    let port = calibrate.port;
    let range = calibrate.range;
    let state = calibrate.state;
    let busy = runner.busy();
    let span = move || {
        let range = range.get();
        format!(
            "{} \u{2013} {}, {} points",
            format_hz(range.start_hz as f64),
            format_hz(range.stop_hz as f64),
            range.points
        )
    };
    let applied = move || state.with(|state| state.as_ref().is_some_and(|state| state.applied));
    let slots: Vec<(u8, String)> = (0..=6).map(|slot| (slot, format!("Slot {slot}"))).collect();
    view! {
        column(class = "tool-stack") {
            row(class = "tool-bar") {
                {button("btn primary", runner.label("reset", "Start over this range", "Starting\u{2026}"), move || busy() || port.with(String::is_empty), move || runner.run("reset", NanoVnaCalStep::Reset))}
                text(class = "tool-mono tool-dim") {{span}}
            }
            {steps("Reflection (CH0)", &ONE_PORT, runner)}
            {steps("Transmission (CH0 \u{2192} CH1), optional", &TWO_PORT, runner)}
            row(class = "tool-bar") {
                {button("btn primary", runner.label("finish", "Apply calibration", "Applying\u{2026}"), move || runner.busy()() || !state.with(|state| reflection_done(state.as_ref())), move || runner.run("finish", NanoVnaCalStep::Finish))}
                {button("btn", move || if applied() { "Switch correction off".to_owned() } else { "Switch correction on".to_owned() }, runner.busy(), move || {
                    let step = if applied() { NanoVnaCalStep::Disable } else { NanoVnaCalStep::Enable };
                    runner.run("toggle", step);
                })}
            }
            row(class = "tool-row") {
                {labelled("Storage", pick(slots, Signal::derive(move || Some(slot.get())), move |picked| slot.set(picked)))}
                {button("btn", runner.label("save", "Save to slot", "Saving\u{2026}"), runner.busy(), move || runner.run("save", NanoVnaCalStep::Save { slot: slot.get_untracked() }))}
                {button("btn", runner.label("recall", "Recall slot", "Recalling\u{2026}"), runner.busy(), move || runner.run("recall", NanoVnaCalStep::Recall { slot: slot.get_untracked() }))}
                {button("btn danger", || "Clear".to_owned(), runner.busy(), move || runner.run("clear", NanoVnaCalStep::Reset))}
            }
            {alert(move || runner.call.error.get())}
            {move || state.get().map(|state| status(&state))}
        }
    }
}

fn steps(title: &'static str, entries: &'static [Step], runner: Runner) -> impl IntoView {
    let state = runner.calibrate.state;
    let rows: Vec<AnyView> = entries
        .iter()
        .map(|entry| {
            let standard = entry.standard;
            let done = move || state.with(|state| state.as_ref().is_some_and(|state| state.standards.contains(&standard)));
            let step = entry.step.clone();
            let key = entry.key;
            AnyView::new(view! {
                row(class = "vna-step") {
                    {button("btn", runner.label(key, entry.label, "Measuring\u{2026}"), runner.busy(), move || runner.run(key, step.clone()))}
                    text(class = "tool-mono", class:ok = done) {{move || if done() { "\u{2713}" } else { "\u{25cb}" }}}
                    text(class = "tool-dim") {{entry.hint}}
                }
            })
        })
        .collect();
    view! {
        column(class = "tool-group") {
            text(class = "legend") {{title}}
            {rows}
        }
    }
}

fn status(state: &NanoVnaCalibration) -> AnyView {
    let standards = if state.standards.is_empty() {
        "none".to_owned()
    } else {
        state
            .standards
            .iter()
            .map(|standard| standard_name(*standard))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let terms = if state.error_terms.is_empty() {
        "none".to_owned()
    } else {
        state.error_terms.join(" ")
    };
    AnyView::new(view! {
        row(class = "tool-chips") {
            {chip("correction", if state.applied { "on" } else { "off" })}
            {chip("standards", standards)}
            {chip("error terms", terms)}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(standards: Vec<NanoVnaStandard>) -> NanoVnaCalibration {
        NanoVnaCalibration {
            port: "COM3".to_owned(),
            standards,
            error_terms: Vec::new(),
            applied: false,
            raw: String::new(),
        }
    }

    #[test]
    fn calibration_applies_only_after_open_short_and_load() {
        assert!(!reflection_done(None));
        assert!(!reflection_done(Some(&state(vec![
            NanoVnaStandard::Open,
            NanoVnaStandard::Short
        ]))));
        assert!(reflection_done(Some(&state(vec![
            NanoVnaStandard::Load,
            NanoVnaStandard::Short,
            NanoVnaStandard::Open,
        ]))));
    }

    #[test]
    fn standards_read_by_their_plain_names() {
        assert_eq!(standard_name(NanoVnaStandard::Isolation), "isolation");
    }
}
