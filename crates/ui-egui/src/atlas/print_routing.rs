//! Atlas fork: the Print dialog follows the printer rules (`pdfcraft_atlas::print_rules`). When it
//! opens it reads the size of the pages to print, preselects the paper and printer the rules give
//! that size, and says why. What the person changes stands for that job. With no rules source
//! (tests of the upstream dialog, the browser) the dialog behaves as it always did.

use std::path::PathBuf;

use pdfcraft_atlas::print_rules::{self, Conflict, LoadedRules, Plan, PrinterRules, Selected};
use pdfcraft_engine::print::SizeMode;

use crate::PdfCraftApp;
use crate::print_ui::{Handling, PrintDraft};
use crate::theme::Tokens;

/// The colour of a warning that a job goes against the rules: legible on the light and dark themes.
const WARNING: egui::Color32 = egui::Color32::from_rgb(196, 110, 0);

/// Where the printer rules come from.
#[derive(Clone, Debug)]
pub(crate) enum RulesSource {
    /// The rules file the app and the command line share, read each time Print opens.
    File(PathBuf),
    /// Rules held in memory, for tests.
    Fixed(PrinterRules),
}

impl RulesSource {
    fn load(&self) -> LoadedRules {
        match self {
            RulesSource::File(path) => PrinterRules::load(path),
            RulesSource::Fixed(rules) => LoadedRules { rules: rules.clone(), problem: None },
        }
    }
}

/// The app's printer-rules state.
#[derive(Default)]
pub(crate) struct PrintRulesState {
    pub(crate) source: Option<RulesSource>,
}

impl PdfCraftApp {
    /// Follow the printer rules in this file (the desktop app's: `print_rules::app_file`).
    pub fn use_print_rules_file(&mut self, path: PathBuf) {
        self.atlas.print.source = Some(RulesSource::File(path));
    }

    /// Follow these printer rules.
    pub fn use_print_rules(&mut self, rules: PrinterRules) {
        self.atlas.print.source = Some(RulesSource::Fixed(rules));
    }
}

/// What the plan was last made from: it is made again when any of this changes.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    pages: Vec<usize>,
    printers: Vec<String>,
    loading: bool,
}

/// The Print dialog's state for following the rules, for one opening of the dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrintRouting {
    /// The rules apply to this print.
    pub active: bool,
    pub rules: PrinterRules,
    /// Why the rules (or some of them) couldn't be read.
    pub rules_problem: Option<String>,
    /// The pages to print, by size.
    pub plan: Plan,
    seen: Option<Seen>,
    /// Why this printer and paper, for the dialog to say.
    pub reason: Option<String>,
    /// No printer may be assumed: Print waits until one is chosen.
    pub must_choose: bool,
    /// The paper before the rules chose one, which is what "last used" still means until a job
    /// is sent on the rules' paper.
    pub last_used_paper: usize,
    /// The paper the rules last put in the dialog.
    applied_paper: Option<usize>,
    /// The person changed the paper for this job.
    pub paper_changed: bool,
    /// The layout the rules last put in the dialog.
    applied_layout: Option<(Handling, SizeMode)>,
    /// The layout before the rules chose Actual size, given back to a job they don't choose it for.
    layout_before: Option<(Handling, SizeMode)>,
    /// The person changed the layout for this job.
    layout_changed: bool,
    /// A job went out on the dialog's paper.
    sent: bool,
    /// How the selected printer and paper go against the rules.
    pub conflict: Option<Conflict>,
}

impl PrintRouting {
    /// The paper to remember as the last one used: what the rules put in the dialog doesn't count
    /// until a job is sent on it. So the setting keeps meaning what the person last used.
    pub(crate) fn paper_to_remember(&self, paper: usize) -> usize {
        if self.active && self.applied_paper.is_some() && !self.paper_changed && !self.sent { self.last_used_paper } else { paper }
    }

    /// The layout to start the next print from, likewise.
    fn layout_to_remember(&self) -> Option<(Handling, SizeMode)> {
        if self.active && !self.layout_changed { self.layout_before } else { None }
    }
}

/// Print has opened: start following the rules, when there are any to follow. The printer picked
/// last time and the paper used last time don't carry over: the pages decide.
pub(crate) fn on_open(app: &mut PdfCraftApp) {
    let Some(source) = &app.atlas.print.source else { return };
    let loaded = source.load();
    let d = &mut app.print_draft;
    d.paper = d.routing.paper_to_remember(d.paper);
    if let Some((handling, size)) = d.routing.layout_to_remember() {
        d.handling = handling;
        d.size = size;
    }
    d.printer_picked = false;
    d.routing =
        PrintRouting { active: true, rules: loaded.rules, rules_problem: loaded.problem, last_used_paper: d.paper, ..PrintRouting::default() };
}

/// Each frame of the dialog: notice what the person changed, and choose again when the pages to
/// print or the printers changed.
pub(crate) fn update(d: &mut PrintDraft, sizes: &[(f64, f64)], labels: &[String]) {
    if !d.routing.active {
        return;
    }
    if d.routing.applied_paper.is_some_and(|paper| paper != d.paper) {
        d.routing.paper_changed = true;
    }
    if d.routing.applied_layout.is_some_and(|layout| layout != (d.handling, d.size)) {
        d.routing.layout_changed = true;
    }
    // A selection that isn't valid yet (a range being typed) keeps the last plan.
    if let Ok(settings) = d.settings(sizes.len(), labels) {
        let seen = Seen { pages: settings.pages, printers: d.printers.iter().map(|p| p.name.clone()).collect(), loading: d.printers_loading };
        if d.routing.seen.as_ref() != Some(&seen) {
            let names: Vec<&str> = seen.printers.iter().map(String::as_str).collect();
            let known = printers_known(d);
            d.routing.plan = print_rules::plan(sizes, &seen.pages, &d.routing.rules, known.then_some(names.as_slice()));
            d.routing.seen = Some(seen);
            apply(d);
        }
    }
    d.routing.conflict = selected_conflict(d);
}

/// Whether the printer list can be believed: until the first one is in, no printer can be called
/// missing.
fn printers_known(d: &PrintDraft) -> bool {
    !d.printers_loading || !d.printers.is_empty()
}

/// How the dialog's printer and paper go against the rules. Save as PDF goes against nothing.
fn selected_conflict(d: &PrintDraft) -> Option<Conflict> {
    let printer = d.printer.as_deref()?;
    let names: Vec<&str> = d.printers.iter().map(|p| p.name.as_str()).collect();
    let selected = Selected { printer, paper: d.paper, one_page_per_sheet: d.handling == Handling::Size };
    print_rules::conflict(d.routing.plan.single(), selected, &d.routing.rules, printers_known(d).then_some(names.as_slice()))
}

/// The one-click fix: the printer and paper the rules want, as if they had chosen them.
fn apply_fix(d: &mut PrintDraft, printer: String, paper: usize) {
    d.printer = Some(printer);
    d.printer_picked = false;
    d.paper = paper;
    d.paper_before_label = None;
    d.routing.paper_changed = false;
    d.routing.applied_paper = Some(paper);
    if d.routing.plan.single().is_some_and(print_rules::SizeGroup::wide_format) && d.handling == Handling::Size {
        d.size = SizeMode::Actual;
    }
    d.routing.layout_changed = false;
    d.routing.applied_layout = Some((d.handling, d.size));
    d.routing.conflict = None;
}

/// What the Print button says, when it isn't plain Print: sending past a warning is a choice.
pub(crate) fn print_label(d: &PrintDraft) -> Option<&'static str> {
    (d.routing.active && d.routing.conflict.is_some()).then_some("Print anyway")
}

/// The warning before sending, with its fix, above the Print button.
pub(crate) fn conflict_strip(ui: &mut egui::Ui, d: &mut PrintDraft) {
    if !d.routing.active {
        return;
    }
    let Some(conflict) = d.routing.conflict.clone() else { return };
    let mut fix = false;
    egui::Frame::new().stroke(egui::Stroke::new(1.0, WARNING)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(conflict.message.as_str()).color(WARNING).strong());
            if let Some(label) = conflict.fix_label() {
                fix = ui.button(label).clicked();
            }
        },
    );
    if fix && let Some((printer, paper)) = conflict.fix {
        apply_fix(d, printer, paper);
    }
    ui.add_space(4.0);
}

/// Put the plan's choice in the dialog, except what the person has changed for this job.
fn apply(d: &mut PrintDraft) {
    let last = d.routing.last_used_paper;
    let Some(group) = d.routing.plan.single() else {
        d.routing.reason =
            (!d.routing.plan.groups.is_empty()).then(|| format!("Mixed page sizes: {}. Choose a printer and paper.", d.routing.plan.breakdown()));
        d.routing.must_choose = d.routing.plan.is_mixed();
        if d.routing.must_choose && !d.printer_picked {
            d.printer = None;
        }
        return;
    };
    let choice = print_rules::choose(group, last);
    d.routing.reason = Some(choice.reason);
    d.routing.must_choose = choice.printer.is_none();
    if !d.printer_picked {
        d.printer = choice.printer;
    }
    if !d.routing.paper_changed {
        d.paper = choice.paper;
        d.paper_before_label = None;
        d.routing.applied_paper = Some(choice.paper);
    }
    if d.routing.layout_changed {
        return;
    }
    if choice.actual {
        if d.routing.layout_before.is_none() {
            d.routing.layout_before = Some((d.handling, d.size));
        }
        (d.handling, d.size) = (Handling::Size, SizeMode::Actual);
        d.routing.applied_layout = Some((d.handling, d.size));
    } else if let Some((handling, size)) = d.routing.layout_before.take() {
        (d.handling, d.size) = (handling, size);
        d.routing.applied_layout = None;
    }
}

/// The person hasn't chosen a printer and the rules can't: the dialog asks.
pub(crate) fn must_choose(d: &PrintDraft) -> bool {
    d.routing.active && d.routing.must_choose && d.printer.is_none() && !d.printer_picked
}

/// Print can't go ahead yet.
pub(crate) fn blocks_print(d: &PrintDraft) -> bool {
    must_choose(d)
}

/// A job is going out with what the dialog holds.
pub(crate) fn before_send(d: &mut PrintDraft) {
    d.routing.sent = true;
}

/// The reason line, under the printer and paper.
pub(crate) fn panel(ui: &mut egui::Ui, d: &mut PrintDraft, t: &Tokens) {
    if !d.routing.active {
        return;
    }
    ui.add_space(4.0);
    if let Some(reason) = &d.routing.reason {
        ui.label(egui::RichText::new(reason.as_str()).strong());
    }
    if d.routing.must_choose && d.printer_picked {
        ui.label(egui::RichText::new("You chose this printer for this job.").small().color(t.text_muted));
    }
    if let Some(problem) = &d.routing.rules_problem {
        ui.label(egui::RichText::new(problem.as_str()).small().color(t.text_muted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_choice_is_not_remembered_as_the_last_paper_until_it_is_used() {
        let mut routing = PrintRouting { active: true, last_used_paper: 3, applied_paper: Some(9), ..PrintRouting::default() };
        assert_eq!(routing.paper_to_remember(9), 3, "only preselected");
        routing.sent = true;
        assert_eq!(routing.paper_to_remember(9), 9, "printed on it");
        routing.sent = false;
        routing.paper_changed = true;
        assert_eq!(routing.paper_to_remember(7), 7, "picked by hand");
        assert_eq!(PrintRouting::default().paper_to_remember(7), 7, "no rules: the upstream setting");
    }
}
